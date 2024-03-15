use std::{
    io::{
        self,
        ErrorKind::{InvalidInput, Other, UnexpectedEof},
        Read, Write,
    },
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use log::{debug, error, info};
use rustls::{ClientConfig, ClientConnection, StreamOwned};

use super::{decoder::Decoder, Agent, StatusError, Url};

#[derive(Copy, Clone)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

pub struct TextRequest {
    inner: Request<StringWriter>,
}

impl TextRequest {
    pub fn new(method: Method, url: Url, data: String, agent: Agent) -> Result<Self> {
        Ok(Self {
            inner: Request::new(StringWriter::default(), method, url, data, agent)?,
        })
    }

    pub fn header(&mut self, header: &str) -> Result<()> {
        self.inner.header(header)
    }

    pub fn text(&mut self) -> Result<&str> {
        self.inner.get_mut().0.clear();
        self.inner.call()?;

        Ok(&self.inner.get_mut().0)
    }
}

pub struct Request<T: Write> {
    stream: Transport,
    handler: Handler<T>,
    raw: String,

    method: Method,
    url: Url,
    headers: String,
    data: String,

    agent: Agent,
}

impl<T: Write> Request<T> {
    pub fn new(writer: T, method: Method, url: Url, data: String, agent: Agent) -> Result<Self> {
        let mut request = Self {
            stream: Transport::new(&url, agent.clone())?,
            handler: Handler::new(writer),
            raw: String::default(),

            method,
            url,
            headers: String::default(),
            data,

            agent,
        };
        request.build()?;

        if !request.data.is_empty() {
            request.header(&format!("Content-Length: {}", request.data.len()))?;
        }

        Ok(request)
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.handler.writer.as_mut().expect("Missing writer")
    }

    pub fn header(&mut self, header: &str) -> Result<()> {
        self.headers = format!(
            "{}\
             {header}\r\n",
            self.headers
        );

        self.build()
    }

    pub fn url(&mut self, url: Url) -> Result<()> {
        if self.url.scheme()? != url.scheme()? || self.url.host()? != url.host()? {
            return self.reconnect(url);
        }

        self.url = url;
        self.build()
    }

    pub fn call(&mut self) -> Result<()> {
        let mut retries = 0;
        loop {
            match self.do_request() {
                Ok(()) => break,
                Err(e) if retries < self.agent.args.retries => {
                    match e.downcast_ref::<io::Error>() {
                        Some(i) if i.kind() == Other => return Err(e),
                        Some(_) => (),
                        _ => return Err(e),
                    }

                    //Don't log first error
                    if retries > 0 {
                        error!("http: {e}, retrying...");
                    }
                    retries += 1;

                    let written = self.handler.written;
                    self.reconnect(self.url.clone())?;

                    if written > 0 {
                        info!("Resuming from offset: {written} bytes");
                        self.handler.resume_target = written;
                    }
                }
                Err(e) => return Err(e),
            }
        }

        self.handler.written = 0;
        self.handler
            .writer
            .as_mut()
            .expect("Missing writer")
            .flush()?;

        Ok(())
    }

    fn do_request(&mut self) -> Result<()> {
        const BUF_SIZE: usize = 2048;

        debug!("Request:\n{}", self.raw);
        self.stream.write_all(self.raw.as_bytes())?;
        self.stream.flush()?;

        //Read into buf and search for the header terminator string,
        //then split buf there and feed remaining half into decoder
        let mut buf = [0u8; BUF_SIZE];
        let mut written = 0;
        let (headers, remaining) = loop {
            let consumed = self.stream.read(&mut buf[written..])?;
            if consumed == 0 {
                return Err(io::Error::from(UnexpectedEof).into());
            }
            written += consumed;

            if let Some(mut headers_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                headers_end += 4; //pass \r\n\r\n
                match (buf.get(..headers_end), buf.get(headers_end..written)) {
                    (Some(headers), Some(remaining)) => {
                        break (String::from_utf8_lossy(headers), remaining);
                    }
                    _ => continue, //loop to return UnexpectedEof
                }
            }
        };
        debug!("Response:\n{headers}");

        let code = headers
            .split_whitespace()
            .nth(1)
            .context("Failed to find request status code")?
            .parse()
            .context("Failed to parse request status code")?;

        match code {
            200 => (),
            _ => return Err(StatusError(code, self.url.clone()).into()),
        }

        match io::copy(
            &mut Decoder::new(remaining.chain(&mut self.stream), &headers)?,
            &mut self.handler,
        ) {
            Ok(_) => Ok(()),
            //Chunk decoder returns InvalidInput on some segment servers, can be ignored
            Err(e) if e.kind() == InvalidInput => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn reconnect(&mut self, url: Url) -> Result<()> {
        debug!("Reconnecting...");
        *self = Request::new(
            self.handler.writer.take().expect("Missing writer"),
            self.method,
            url,
            self.data.clone(),
            self.agent.clone(),
        )?;

        Ok(())
    }

    fn build(&mut self) -> Result<()> {
        self.raw = format!(
            "{method} /{path} HTTP/1.1\r\n\
             Host: {host}\r\n\
             User-Agent: {user_agent}\r\n\
             Accept: */*\r\n\
             Accept-Language: en-US\r\n\
             Accept-Encoding: gzip\r\n\
             Connection: keep-alive\r\n\
             {headers}\r\n\
             {data}",
            method = self.method.as_str(),
            path = self.url.path()?,
            host = self.url.host()?,
            user_agent = &self.agent.args.user_agent,
            headers = self.headers,
            data = self.data,
        );

        Ok(())
    }
}

#[allow(clippy::large_enum_variant)]
enum Transport {
    Http(TcpStream),
    Https(StreamOwned<ClientConnection, TcpStream>),
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
    fn new(url: &Url, agent: Agent) -> Result<Self> {
        let scheme = url.scheme()?;
        let host = url.host()?;
        let port = url.port()?;

        if agent.args.force_https {
            ensure!(
                scheme == "https",
                "URL protocol is not HTTPS and --force-https is enabled: {url}",
            );
        }

        let addrs = (host, port).to_socket_addrs()?;
        let sock = if agent.args.force_ipv4 {
            Self::try_connect(addrs.filter(SocketAddr::is_ipv4), agent.args.timeout)?
        } else {
            Self::try_connect(addrs, agent.args.timeout)?
        };

        sock.set_nodelay(true)?;
        sock.set_read_timeout(Some(agent.args.timeout))?;
        sock.set_write_timeout(Some(agent.args.timeout))?;

        match scheme {
            "http" => Ok(Self::Http(sock)),
            "https" => Ok(Self::Https(Self::init_tls(host, sock, agent.tls_config)?)),
            _ => bail!("{scheme} is not supported"),
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

        Err(io_error.expect("Missing io error while connection failed"))
    }

    fn init_tls(
        host: &str,
        mut sock: TcpStream,
        tls_config: Arc<ClientConfig>,
    ) -> Result<StreamOwned<ClientConnection, TcpStream>> {
        let mut conn = ClientConnection::new(tls_config, host.to_owned().try_into()?)?;
        conn.complete_io(&mut sock)?; //handshake

        Ok(StreamOwned::new(conn, sock))
    }
}

#[derive(Default)]
struct StringWriter(String);

impl Write for StringWriter {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        unimplemented!();
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.push_str(&String::from_utf8_lossy(buf));
        Ok(())
    }
}

struct Handler<T: Write> {
    writer: Option<T>,

    written: usize,
    resume_target: usize,
}

impl<T: Write> Write for Handler<T> {
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

        self.writer
            .as_mut()
            .expect("Missing writer")
            .write_all(buf)?;

        self.written += buf.len(); //len of the potential trimmed buf reference
        Ok(buf_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<T: Write> Handler<T> {
    fn new(writer: T) -> Self {
        Self {
            writer: Some(writer),

            written: usize::default(),
            resume_target: usize::default(),
        }
    }
}
