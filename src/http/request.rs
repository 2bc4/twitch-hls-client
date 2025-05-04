use std::{
    fmt::Arguments,
    hash::{DefaultHasher, Hasher},
    io::{
        self,
        ErrorKind::{InvalidInput, Other, UnexpectedEof},
        Read, Write,
    },
    mem,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    str,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use log::{debug, error};
use rustls::{ClientConnection, StreamOwned};

use super::{Agent, Method, Scheme, StatusError, Url, decoder::Decoder};

pub struct Request<W: Write> {
    writer: W,

    stream: Option<Transport>,
    scheme: Scheme,
    host_hash: u64,

    headers_buf: Box<[u8]>,
    retries: u64,
    agent: Agent,
}

impl<W: Write> Request<W> {
    const HEADERS_BUF_SIZE: usize = 2048;

    pub fn new(writer: W, agent: Agent) -> Self {
        Self {
            writer,
            headers_buf: vec![0u8; Self::HEADERS_BUF_SIZE].into_boxed_slice(),
            retries: agent.args.retries,
            agent,
            stream: Option::default(),
            scheme: Scheme::default(),
            host_hash: u64::default(),
        }
    }

    pub fn into_writer(self) -> W {
        self.writer
    }

    pub fn into_text_request(self) -> TextRequest {
        let mut request = self.agent.text();
        request.0.stream = self.stream;
        request.0.scheme = self.scheme;
        request.0.host_hash = self.host_hash;

        request
    }

    pub const fn get_ref(&self) -> &W {
        &self.writer
    }

    pub fn call(&mut self, method: Method, url: &Url) -> Result<()> {
        self.call_impl(method, url, None)
    }

    fn call_impl(&mut self, method: Method, url: &Url, args: Option<Arguments>) -> Result<()> {
        let host = url.host()?;
        let hash = Self::hash(host);
        if self.stream.is_none() || self.host_hash != hash || self.scheme != url.scheme {
            self.connect(url, host, hash)?;
        }

        let mut retries = 0;
        loop {
            match self.converse(method, host, url, args) {
                Ok(()) => break,
                Err(e) if retries < self.retries => {
                    match e.downcast_ref::<io::Error>() {
                        Some(i) if i.kind() == Other => return Err(e),
                        Some(_) => (),
                        _ => return Err(e),
                    }

                    if retries == 0 {
                        debug!("got {e}");
                    } else {
                        error!("http: {e}, retrying...");
                    }

                    retries += 1;
                    self.connect(url, host, hash)?;
                }
                Err(e) => return Err(e),
            }
        }

        self.writer.flush()?;
        Ok(())
    }

    fn converse(
        &mut self,
        method: Method,
        host: &str,
        url: &Url,
        args: Option<Arguments>,
    ) -> Result<()> {
        let mut stream = self.stream.as_mut().expect("Missing stream while writing");
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
            user_agent = &self.agent.args.user_agent,
            args = args.unwrap_or(format_args!("\r\n"))
        )?;
        stream.flush()?;

        let mut written = 0;
        let (headers, remaining) = loop {
            let consumed = stream.read(&mut self.headers_buf[written..])?;
            if consumed == 0 {
                return Err(io::Error::from(UnexpectedEof).into());
            }
            written += consumed;

            if let Some((headers, remaining)) = self
                .headers_buf
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .and_then(|p| {
                    self.headers_buf[..written].split_at_mut_checked(p + 4 /* pass \r\n\r\n */)
                })
            {
                headers.make_ascii_lowercase();
                break (str::from_utf8(headers)?, remaining);
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
            &mut self.writer,
        ) {
            Ok(_) => Ok(()),
            //Chunk decoder returns InvalidInput on some segment servers, can be ignored
            Err(e) if e.kind() == InvalidInput => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn connect(&mut self, url: &Url, host: &str, host_hash: u64) -> Result<()> {
        debug!("Connecting to {host}...");
        self.stream = Some(Transport::new(url, host, &self.agent)?);
        self.scheme = url.scheme;
        self.host_hash = host_hash;

        Ok(())
    }

    fn hash(host: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        hasher.write(host.as_bytes());

        hasher.finish()
    }
}

pub struct TextRequest(Request<StringWriter>);

impl TextRequest {
    pub fn new(agent: Agent) -> Self {
        Self(Request::new(StringWriter::default(), agent))
    }

    pub fn take(&mut self) -> String {
        mem::take(&mut self.0.writer.0)
    }

    pub fn text(&mut self, method: Method, url: &Url) -> Result<&str> {
        self.text_impl(method, url, None)
    }

    pub fn text_fmt(&mut self, method: Method, url: &Url, args: Arguments) -> Result<&str> {
        self.text_impl(method, url, Some(args))
    }

    fn text_impl(&mut self, method: Method, url: &Url, data: Option<Arguments>) -> Result<&str> {
        self.0.writer.0.clear();
        self.0.call_impl(method, url, data)?;

        Ok(&self.0.writer.0)
    }
}

enum Transport {
    Tls(Box<StreamOwned<ClientConnection, TcpStream>>),
    Unencrypted(TcpStream),
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Tls(stream) => stream.read(buf),
            Self::Unencrypted(sock) => sock.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Tls(stream) => stream.flush(),
            Self::Unencrypted(sock) => sock.flush(),
        }
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        match self {
            Self::Tls(stream) => stream.write_all(buf),
            Self::Unencrypted(sock) => sock.write_all(buf),
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
            Scheme::Http => Ok(Self::Unencrypted(sock)),
            Scheme::Https => Ok(Self::Tls(Box::new(StreamOwned::new(
                ClientConnection::new(agent.tls_config.clone(), host.to_owned().try_into()?)?,
                sock,
            )))),
            Scheme::Unknown => bail!("Unsupported protocol"),
        }
    }

    fn try_connect(iter: impl Iterator<Item = SocketAddr>, timeout: Duration) -> Result<TcpStream> {
        let mut addrs = iter.peekable();
        ensure!(addrs.peek().is_some(), "Failed to resolve socket address");

        let mut io_error = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, timeout) {
                Ok(sock) => return Ok(sock),
                Err(e) => io_error = Some(e),
            }
        }

        Err(io_error
            .expect("Missing IO error while connection failed")
            .into())
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
        self.0.push_str(
            str::from_utf8(buf)
                .map_err(|e| io::Error::other(format!("HTTP response wasn't valid utf-8: {e}")))?,
        );

        Ok(())
    }
}
