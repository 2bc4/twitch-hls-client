use std::{
    fmt::Arguments,
    hash::{DefaultHasher, Hasher},
    io::{self, Read, Write},
    mem,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    str,
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use log::{debug, error};
use rustls::{ClientConfig, ClientConnection, StreamOwned};
use rustls_platform_verifier::ConfigVerifierExt;

use super::{
    MAX_HEADERS_SIZE, Method, Scheme, StatusError, Url, decoder::Decoder, parse_status, proxy,
};

use crate::config::Config;

static TLS_CONFIG: OnceLock<Arc<ClientConfig>> = OnceLock::new();

pub fn load_certificates() -> Result<()> {
    debug!("Loading TLS root certificates...");
    let mut config = ClientConfig::with_platform_verifier()
        .context("Failed to initialize TLS certificate store")?;

    config.alpn_protocols = vec![b"http/1.1".to_vec()];

    TLS_CONFIG
        .set(Arc::new(config))
        .expect("TLS config already initialized");

    Ok(())
}

pub struct Request<W: Write> {
    writer: W,

    stream: Option<Transport>,
    scheme: Scheme,
    host_hash: u64,

    headers_buf: Box<[u8]>,
    decode_buf: Box<[u8]>,
    no_retry: bool,
    retries: u64,

    http_proxy: Option<Url>,
}

impl<W: Write> Request<W> {
    const DECODE_BUF_SIZE: usize = 16 * 1024; //rustls with default settings returns up to 16kb

    pub fn new(writer: W) -> Self {
        Self {
            writer,
            headers_buf: vec![0u8; MAX_HEADERS_SIZE].into_boxed_slice(),
            decode_buf: vec![0u8; Self::DECODE_BUF_SIZE].into_boxed_slice(),
            retries: Config::get().retries,
            stream: Option::default(),
            scheme: Scheme::default(),
            host_hash: u64::default(),
            no_retry: bool::default(),
            http_proxy: Option::default(),
        }
    }

    pub fn into_writer(self) -> W {
        self.writer
    }

    pub const fn get_ref(&self) -> &W {
        &self.writer
    }

    pub const fn get_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    pub fn call(&mut self, method: Method, url: &Url) -> Result<()> {
        self.call_impl(method, url, None).map_err(map_timeout)
    }

    fn call_impl(&mut self, method: Method, url: &Url, args: Option<Arguments>) -> Result<()> {
        let host = url.host()?;
        let hash = Self::hash(host);
        if self.stream.is_none() || self.host_hash != hash || self.scheme != url.scheme {
            self.connect(url, hash)?;
        }

        let mut retries = 0;
        loop {
            match self.converse(method, host, url, args) {
                Ok(()) => break,
                Err(error)
                    if !self.no_retry && retries < self.retries && Self::should_retry(&error) =>
                {
                    if retries > 0 {
                        error!("http: {error}, retrying...");
                    }

                    retries += 1;
                    self.connect(url, hash)?;
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
            path = url.path(),
            user_agent = Config::get().user_agent,
            args = args.unwrap_or_else(|| format_args!("\r\n"))
        )?;
        stream.flush()?;

        let (headers, body) = Self::read_headers(stream, &mut self.headers_buf)?;
        debug!("Response:\n{headers}");

        let status = parse_status(headers)?;
        if status != 200 {
            return Err(StatusError(status, url.clone()).into());
        }

        match method {
            Method::Get | Method::Post => {
                let mut decoder = Decoder::new(body.chain(&mut stream), headers)?;
                loop {
                    let read = decoder.read(&mut self.decode_buf)?;
                    if read == 0 {
                        break Ok(());
                    }

                    self.writer.write_all(&self.decode_buf[..read])?;
                }
            }
            Method::Head => Ok(()),
        }
    }

    fn connect(&mut self, url: &Url, host_hash: u64) -> Result<()> {
        self.stream = Some(Transport::new(url, self.http_proxy.as_ref())?);
        self.scheme = url.scheme;
        self.host_hash = host_hash;

        Ok(())
    }

    fn read_headers<'a>(stream: &mut Transport, buf: &'a mut [u8]) -> Result<(&'a str, &'a [u8])> {
        let mut written = 0;
        loop {
            let read = stream.read(&mut buf[written..])?;
            if read == 0 {
                return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
            }
            written += read;

            let pos = buf[..written].windows(4).position(|w| w == b"\r\n\r\n");
            if let Some(pos) = pos {
                let (headers, body) = buf[..written].split_at_mut(pos + 4);
                headers.make_ascii_lowercase();

                break Ok((str::from_utf8(headers)?, body));
            }
        }
    }

    fn hash(host: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        hasher.write(host.as_bytes());

        hasher.finish()
    }

    fn should_retry(error: &anyhow::Error) -> bool {
        error.is::<StatusError>() && !StatusError::is_not_found(error)
            || error
                .downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() != io::ErrorKind::Other)
    }
}

pub struct TextRequest(Request<StringWriter>);

impl TextRequest {
    pub fn new() -> Self {
        Self(Request::new(StringWriter::default()))
    }

    pub fn take(&mut self) -> String {
        mem::take(&mut self.0.writer.0)
    }

    pub fn text(&mut self, method: Method, url: &Url) -> Result<&str> {
        self.text_impl(method, url, None)
    }

    pub fn text_no_retry(&mut self, method: Method, url: &Url) -> Result<()> {
        self.0.no_retry = true;
        self.text_impl(method, url, None)?;
        self.0.no_retry = false;

        Ok(())
    }

    pub fn text_fmt(&mut self, method: Method, url: &Url, args: Arguments) -> Result<&str> {
        self.text_impl(method, url, Some(args))
    }

    pub fn set_http_proxy(&mut self, http_proxy: &Url) {
        self.0.http_proxy = Some(http_proxy.clone());
    }

    fn text_impl(&mut self, method: Method, url: &Url, data: Option<Arguments>) -> Result<&str> {
        self.0.writer.0.clear();
        self.0.call_impl(method, url, data).map_err(map_timeout)?;

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
    fn new(url: &Url, http_proxy: Option<&Url>) -> Result<Self> {
        let cfg = Config::get();

        ensure!(
            !cfg.force_https || url.scheme == Scheme::Https,
            "URL protocol is not HTTPS and --force-https is enabled: {url}",
        );

        let host = url.host()?;
        let port = url.port()?;

        let socks5_addrs = cfg.socks5.as_ref().filter(|_| {
            let real_host = http_proxy
                .as_ref()
                .and_then(|p| p.host().ok())
                .unwrap_or(host);

            cfg.socks5_restrict
                .as_ref()
                .is_none_or(|h| h.iter().any(|h| h == real_host))
        });

        let sock = match (socks5_addrs, http_proxy) {
            (None, None) => {
                debug!("Connecting to {host}...");
                Self::connect(&Self::resolve(host, port)?, cfg.force_ipv4, cfg.timeout)?
            }
            (Some(socks5_addrs), None) => {
                debug!("Connecting to {host} via socks5 proxy...");
                proxy::socks5_connect(
                    Self::connect(socks5_addrs, cfg.force_ipv4, cfg.timeout)?,
                    host,
                    port,
                )?
            }
            (None, Some(http_proxy)) => {
                debug!("Connecting to {host} via http proxy...");
                proxy::http_connect(
                    Self::connect(
                        &Self::resolve(http_proxy.host()?, http_proxy.port()?)?,
                        cfg.force_ipv4,
                        cfg.timeout,
                    )?,
                    host,
                    port,
                )?
            }
            (Some(socks5_addrs), Some(http_proxy)) => {
                debug!("Connecting to {host} via socks5 proxy -> http proxy...");
                proxy::http_connect(
                    proxy::socks5_connect(
                        Self::connect(socks5_addrs, cfg.force_ipv4, cfg.timeout)?,
                        http_proxy.host()?,
                        http_proxy.port()?,
                    )?,
                    host,
                    port,
                )?
            }
        };

        match url.scheme {
            Scheme::Http => Ok(Self::Unencrypted(sock)),
            Scheme::Https => {
                let tls_conn = ClientConnection::new(
                    TLS_CONFIG
                        .get()
                        .expect("TLS config not initialized")
                        .clone(),
                    host.to_owned().try_into()?,
                )?;

                Ok(Self::Tls(Box::new(StreamOwned::new(tls_conn, sock))))
            }
            Scheme::Unknown | Scheme::HttpProxy => bail!("Unsupported protocol"),
        }
    }

    fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        Ok((host, port).to_socket_addrs()?.collect())
    }

    fn connect(addrs: &[SocketAddr], force_ipv4: bool, timeout: Duration) -> Result<TcpStream> {
        ensure!(!addrs.is_empty(), "Failed to resolve socket address");

        let mut error = None;
        for addr in addrs
            .iter()
            .filter(|a| !force_ipv4 || SocketAddr::is_ipv4(a))
        {
            match TcpStream::connect_timeout(addr, timeout) {
                Ok(sock) => {
                    sock.set_nodelay(true)?;
                    sock.set_read_timeout(Some(timeout))?;
                    sock.set_write_timeout(Some(timeout))?;

                    return Ok(sock);
                }
                Err(e) => error = Some(e),
            }
        }

        Err(error
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
                .map_err(|e| io::Error::other(format!("HTTP response wasn't valid UTF-8: {e}")))?,
        );

        Ok(())
    }
}

fn map_timeout(err: anyhow::Error) -> anyhow::Error {
    if let Some(io_err) = err.downcast_ref::<io::Error>()
        && io_err.kind() == io::ErrorKind::WouldBlock
    {
        return io::Error::from(io::ErrorKind::TimedOut).into();
    }

    err
}
