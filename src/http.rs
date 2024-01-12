use std::{
    fmt,
    io::{self, Write},
    str,
};

use anyhow::{ensure, Result};
use curl::easy::{Easy2, Handler, InfoType, IpResolve, List, WriteError};
use log::debug;
use url::Url;

use crate::{constants, ARGS};

#[derive(Debug)]
pub enum Error {
    Status(u32, String),
    NotFound(String),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Status(code, url) => write!(f, "Status code {code} on {url}"),
            Self::NotFound(url) => write!(f, "Not found: {url}"),
        }
    }
}

pub struct TextRequest {
    request: Request<Vec<u8>>,
}

impl TextRequest {
    pub fn get(url: &Url) -> Result<Self> {
        let mut request = Request::new(Vec::new(), url)?;
        request.handle.get(true)?;

        Ok(Self { request })
    }

    pub fn post(url: &Url, data: &str) -> Result<Self> {
        let mut request = Request::new(Vec::new(), url)?;
        request.handle.post(true)?;
        request.handle.post_fields_copy(data.as_bytes())?;

        Ok(Self { request })
    }

    pub fn header(&mut self, header: &str) -> Result<()> {
        let mut list = List::new();
        list.append(header)?;

        self.request.handle.http_headers(list)?;
        Ok(())
    }

    pub fn text(&mut self) -> Result<String> {
        self.request.perform()?;

        let text = String::from_utf8_lossy(self.request.get_ref()).to_string();
        self.request.get_mut().clear();

        Ok(text)
    }
}

pub struct WriterRequest<T>
where
    T: Write,
{
    request: Request<T>,
}

impl<T: Write> WriterRequest<T> {
    pub fn get(writer: T, url: &Url) -> Result<Self> {
        let mut request = Request::new(writer, url)?;
        request.handle.get(true)?;

        Ok(Self { request })
    }

    pub fn url(&mut self, url: &Url) -> Result<()> {
        self.request.url(url)
    }

    pub fn call(&mut self) -> Result<()> {
        self.request.perform()
    }
}

struct Request<T>
where
    T: Write,
{
    handle: Easy2<RequestHandler<T>>,
}

impl<T: Write> Request<T> {
    pub fn new(writer: T, url: &Url) -> Result<Self> {
        let mut request = Self {
            handle: Easy2::new(RequestHandler {
                writer,
                error: Option::default(),
            }),
        };

        let args = ARGS.get().unwrap();
        if args.force_ipv4 {
            request.handle.ip_resolve(IpResolve::V4)?;
        }

        request.handle.verbose(args.debug)?;
        request.handle.timeout(args.http_timeout)?;
        request.handle.tcp_nodelay(true)?;
        request.handle.accept_encoding("")?;
        request.handle.useragent(constants::USER_AGENT)?;
        request.url(url)?;
        Ok(request)
    }

    pub fn perform(&mut self) -> Result<()> {
        let args = ARGS.get().unwrap();
        let mut retries = 0;
        loop {
            match self.handle.perform() {
                Ok(()) => break,
                Err(_) if retries < args.http_retries => retries += 1,
                Err(e) => return Err(e.into()),
            }
        }

        self.handle
            .get_ref()
            .error
            .as_ref()
            .map_or_else(|| Ok(()), |e| Err(io::Error::from(e.kind())))?;

        let code = self.handle.response_code()?;
        match code {
            200 => Ok(()),
            404 => Err(Error::NotFound(self.handle.effective_url()?.unwrap().to_owned()).into()),
            _ => Err(Error::Status(code, self.handle.effective_url()?.unwrap().to_owned()).into()),
        }
    }

    pub fn url(&mut self, url: &Url) -> Result<()> {
        if ARGS.get().unwrap().force_https {
            ensure!(
                url.scheme() == "https",
                "URL protocol is not HTTPS and --force-https is enabled: {url}"
            );
        }

        self.handle.url(url.as_ref())?;
        Ok(())
    }

    pub fn get_ref(&self) -> &T {
        &self.handle.get_ref().writer
    }

    pub fn get_mut(&mut self) -> &mut T {
        &mut self.handle.get_mut().writer
    }
}

struct RequestHandler<T>
where
    T: Write,
{
    writer: T,
    error: Option<io::Error>,
}

impl<T: Write> Handler for RequestHandler<T> {
    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        if let Err(e) = self.writer.write_all(data) {
            self.error = Some(e);
        }

        Ok(data.len())
    }

    fn debug(&mut self, kind: InfoType, data: &[u8]) {
        if !matches!(kind, InfoType::Text) {
            return;
        }

        let text = String::from_utf8_lossy(data);
        if text.starts_with("Found bundle") || text.starts_with("Can not multiplex") {
            return;
        }

        #[cfg(target_os = "windows")]
        if text.starts_with("schannel: failed to decrypt data") {
            return;
        }

        debug!("{}", text.strip_suffix('\n').unwrap_or(&text));
    }
}
