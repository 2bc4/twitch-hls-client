use std::{
    fmt::{self, Arguments, Display, Formatter},
    hash::{DefaultHasher, Hasher},
    io::{
        self,
        ErrorKind::{InvalidInput, Other, UnexpectedEof},
        Read, Write,
    },
    mem,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use log::{debug, error, info};
use rustls::{ClientConnection, StreamOwned};

use super::{decoder::Decoder, Agent, Scheme, StatusError, Url};

#[derive(Default, Copy, Clone)]
pub enum Method {
    #[default]
    Get,
    Post,
}

impl Display for Method {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::Get => f.write_str("GET"),
            Self::Post => f.write_str("POST"),
        }
    }
}

pub struct TextRequest(Request<StringWriter>);

impl TextRequest {
    pub fn new(agent: Agent) -> Self {
        Self(Request::new(StringWriter::default(), agent))
    }

    pub fn take(&mut self) -> String {
        mem::take(&mut self.0.handler.writer.0)
    }

    pub fn text(&mut self, method: Method, url: &Url) -> Result<&str> {
        self.text_impl(method, url, None)
    }

    pub fn text_fmt(&mut self, method: Method, url: &Url, args: Arguments) -> Result<&str> {
        self.text_impl(method, url, Some(args))
    }

    fn text_impl(&mut self, method: Method, url: &Url, data: Option<Arguments>) -> Result<&str> {
        self.0.handler.writer.0.clear();
        self.0.call_impl(method, url, data)?;

        Ok(&self.0.handler.writer.0)
    }
}

pub struct Request<W: Write> {
    handler: Handler<W>,

    stream: Option<Transport>,
    scheme: Scheme,
    hash: u64,

    retries: u64,
    agent: Agent,
}

impl<W: Write> Request<W> {
    pub fn new(writer: W, agent: Agent) -> Self {
        Self {
            handler: Handler::new(writer),
            retries: agent.args.retries,
            agent,
            stream: Option::default(),
            scheme: Scheme::default(),
            hash: u64::default(),
        }
    }

    pub fn into_text_request(self) -> TextRequest {
        let mut request = self.agent.text();
        request.0.stream = self.stream;
        request.0.scheme = self.scheme;
        request.0.hash = self.hash;

        request
    }

    pub fn call(&mut self, method: Method, url: &Url) -> Result<()> {
        self.call_impl(method, url, None)
    }

    fn call_impl(&mut self, method: Method, url: &Url, args: Option<Arguments>) -> Result<()> {
        let host = url.host()?;
        let hash = Self::hash_host(host);
        if self.stream.is_none() || self.hash != hash || self.scheme != url.scheme {
            self.connect(url, host, hash)?;
        }

        let mut retries = 0;
        loop {
            match self.converse(method, url, args) {
                Ok(()) => break,
                Err(e) if retries < self.retries => {
                    match e.downcast_ref::<io::Error>() {
                        Some(i) if i.kind() == Other => return Err(e),
                        Some(_) => (),
                        _ => return Err(e),
                    }

                    //Don't log first error
                    if retries > 0 {
                        error!("http: {e}, retrying...");
                    } else {
                        debug!("got {e}");
                    }
                    retries += 1;

                    let written = self.handler.written;
                    self.connect(url, host, hash)?;

                    if written > 0 {
                        info!("Resuming from offset: {written} bytes");
                        self.handler.resume_target = written;
                    }
                }
                Err(e) => return Err(e),
            }
        }

        self.handler.written = 0;
        self.handler.writer.flush()?;

        Ok(())
    }

    fn converse(&mut self, method: Method, url: &Url, args: Option<Arguments>) -> Result<()> {
        let mut stream = self.stream.as_mut().expect("Missing stream");
        write!(
            stream,
            "{method} /{path} HTTP/1.1\r\n\
             Host: {host}\r\n\
             User-Agent: {user_agent}\r\n\
             Accept: */*\r\n\
             Accept-Language: en-US\r\n\
             Accept-Encoding: gzip\r\n\
             Connection: keep-alive\r\n\
             {args}",
            path = url.path()?,
            host = url.host()?,
            user_agent = &self.agent.args.user_agent,
            args = args.unwrap_or(format_args!("\r\n")),
        )?;
        stream.flush()?;

        //Read into buf and search for the header terminator string,
        //then split buf there and feed remaining half into decoder
        let mut buf = [0u8; 2048];
        let mut written = 0;
        let (headers, remaining) = loop {
            let consumed = stream.read(&mut buf[written..])?;
            if consumed == 0 {
                return Err(io::Error::from(UnexpectedEof).into());
            }
            written += consumed;

            if let Some((headers, remaining)) = buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .and_then(|p| buf.split_at_mut_checked(p + 4 /* pass \r\n\r\n */))
                .and_then(|(h, r)| {
                    let len = written - h.len();
                    Some((h, r.get(..len)?))
                })
            {
                headers.make_ascii_lowercase();

                //If any of the headers aren't valid UTF-8 it's garbage so just grab first
                let Some(headers) = headers.utf8_chunks().next() else {
                    bail!("Response wasn't valid UTF-8");
                };

                break (headers.valid(), remaining);
            }
        };
        debug!("Response:\n{headers}");

        let code = headers
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .context("Failed to parse HTTP status code")?;

        if code != 200 {
            return Err(StatusError(code, url.clone()).into());
        }

        match io::copy(
            &mut Decoder::new(remaining.chain(&mut stream), headers)?,
            &mut self.handler,
        ) {
            Ok(_) => Ok(()),
            //Chunk decoder returns InvalidInput on some segment servers, can be ignored
            Err(e) if e.kind() == InvalidInput => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn hash_host(host: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        hasher.write(host.as_bytes());

        hasher.finish()
    }

    fn connect(&mut self, url: &Url, host: &str, hash: u64) -> Result<()> {
        debug!("Connecting to {host}...");
        self.stream = Some(Transport::new(url, host, &self.agent)?);
        self.scheme = url.scheme;
        self.hash = hash;

        Ok(())
    }
}

enum Transport {
    Http(TcpStream),
    Https(Box<StreamOwned<ClientConnection, TcpStream>>),
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Http(sock) => sock.read(buf),
            Self::Https(stream) => stream.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Self::Http(sock) => sock.write(buf),
            Self::Https(stream) => stream.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Http(sock) => sock.flush(),
            Self::Https(stream) => stream.flush(),
        }
    }
}

impl Transport {
    fn new(url: &Url, host: &str, agent: &Agent) -> Result<Self> {
        if agent.args.force_https {
            ensure!(
                url.scheme == Scheme::Https,
                "URL protocol is not HTTPS and --force-https is enabled: {url}",
            );
        }

        let addrs = (host, url.port()?).to_socket_addrs()?;
        let sock = if agent.args.force_ipv4 {
            Self::try_connect(addrs.filter(SocketAddr::is_ipv4), agent.args.timeout)?
        } else {
            Self::try_connect(addrs, agent.args.timeout)?
        };

        sock.set_nodelay(true)?;
        sock.set_read_timeout(Some(agent.args.timeout))?;
        sock.set_write_timeout(Some(agent.args.timeout))?;

        match url.scheme {
            Scheme::Http => Ok(Self::Http(sock)),
            Scheme::Https => Ok(Self::Https(Box::new(StreamOwned::new(
                ClientConnection::new(agent.tls_config.clone(), host.to_owned().try_into()?)?,
                sock,
            )))),
            Scheme::Unknown => bail!("Unsupported protocol"),
        }
    }

    fn try_connect(
        iter: impl Iterator<Item = SocketAddr>,
        timeout: Duration,
    ) -> Result<TcpStream, io::Error> {
        let mut io_error = None;
        for addr in iter {
            match TcpStream::connect_timeout(&addr, timeout) {
                Ok(sock) => return Ok(sock),
                Err(e) => io_error = Some(e),
            }
        }

        Err(io_error.expect("Missing IO error while connection failed"))
    }
}

#[derive(Default)]
struct StringWriter(String);

impl Write for StringWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        for chunk in buf.utf8_chunks() {
            self.0.push_str(chunk.valid());
        }

        Ok(())
    }
}

struct Handler<W: Write> {
    writer: W,
    written: usize,
    resume_target: usize,
}

impl<W: Write> Write for Handler<W> {
    fn write(&mut self, mut buf: &[u8]) -> io::Result<usize> {
        let buf_len = buf.len();
        if self.resume_target > 0 {
            if (self.written + buf_len) >= self.resume_target {
                buf = &buf[self.resume_target - self.written..];
                self.resume_target = 0;
            } else {
                self.written += buf_len;
                return Ok(buf_len); //throw buf into the void
            }
        }

        self.writer.write_all(buf)?;
        self.written += buf.len(); //len of the potential trimmed buf reference

        Ok(buf_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<W: Write> Handler<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            written: usize::default(),
            resume_target: usize::default(),
        }
    }
}
