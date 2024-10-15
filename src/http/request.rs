use std::{
    fmt::Arguments,
    hash::{DefaultHasher, Hasher},
    io::{
        self, BufRead, BufReader,
        ErrorKind::{InvalidData, Other, UnexpectedEof},
        Read, Write,
    },
    mem,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    str,
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use log::{debug, error, info};

use super::{
    decoder::Decoder,
    tls_stream::{TlsStream, TLS_MAX_FRAG_SIZE},
    Agent, Method, Scheme, StatusError, Url,
};

pub struct Request<W: Write> {
    handler: Handler<W>,

    stream: Option<BufReader<Transport>>,
    scheme: Scheme,
    hash: u64,

    decoded_buf: Box<[u8]>,
    retries: u64,
    agent: Agent,
}

impl<W: Write> Request<W> {
    pub fn new(writer: W, agent: Agent) -> Self {
        Self {
            handler: Handler::new(writer),
            decoded_buf: vec![0u8; TLS_MAX_FRAG_SIZE].into_boxed_slice(),
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

                    self.connect(url, host, hash)?;
                    if self.handler.written > 0 {
                        info!("Resuming from offset: {} bytes", self.handler.written);
                        self.handler.resume_target = self.handler.written;
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
            stream.get_mut(),
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
        stream.get_mut().flush()?;

        let (headers, headers_len) = loop {
            let buf = stream.fill_buf()?;
            if buf.is_empty() {
                return Err(io::Error::from(UnexpectedEof).into());
            }

            if let Some(mut position) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                position += 4; //pass \r\n\r\n
                break (str::from_utf8(&buf[..position])?, position);
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

        let mut decoder = Decoder::new(headers);
        stream.consume(headers_len);
        decoder.set_reader(&mut stream)?;

        loop {
            let consumed = decoder.read(&mut self.decoded_buf)?;
            if consumed == 0 {
                break Ok(());
            }

            self.handler.write_all(&self.decoded_buf[..consumed])?;
        }
    }

    fn connect(&mut self, url: &Url, host: &str, hash: u64) -> Result<()> {
        debug!("Connecting to {host}...");

        self.stream = Some(BufReader::with_capacity(
            TLS_MAX_FRAG_SIZE,
            Transport::new(url, host, &self.agent)?,
        ));
        self.scheme = url.scheme;
        self.hash = hash;

        Ok(())
    }

    fn hash_host(host: &str) -> u64 {
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

enum Transport {
    Tls(Box<TlsStream>),
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
            Scheme::Https => Ok(Self::Tls(Box::new(TlsStream::new(sock, host, agent)?))),
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
        match str::from_utf8(buf) {
            Ok(string) => {
                self.0.push_str(string);
                Ok(())
            }
            Err(_) => Err(io::Error::from(InvalidData)),
        }
    }
}

struct Handler<W: Write> {
    writer: W,
    written: usize,
    resume_target: usize,
}

impl<W: Write> Write for Handler<W> {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unreachable!();
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write_all(&mut self, mut buf: &[u8]) -> io::Result<()> {
        let buf_len = buf.len();
        if self.resume_target > 0 {
            if (self.written + buf_len) >= self.resume_target {
                buf = &buf[self.resume_target - self.written..];
                self.resume_target = 0;
            } else {
                self.written += buf_len;
                return Ok(()); //throw buf into the void
            }
        }

        self.writer.write_all(buf)?;
        self.written += buf.len(); //len of the potential trimmed buf reference

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
