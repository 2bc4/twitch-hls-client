use std::{
    fmt,
    io::{self, Write},
    str,
    time::Duration,
};

use anyhow::Result;
use curl::easy::{Easy2, Handler, InfoType, List, WriteError};
use log::debug;
use url::Url;

use crate::constants;

#[derive(Debug)]
pub enum Error {
    Status(u32, String),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Status(code, url) => write!(f, "Status code {code} on {url}"),
        }
    }
}

fn init_curl<T: Write>(handle: &mut Easy2<RequestHandler<T>>, url: &Url) -> Result<()> {
    handle.verbose(log::max_level() == log::LevelFilter::Debug)?;
    handle.connect_timeout(Duration::from_secs(constants::HTTP_CONNECT_TIMEOUT_SECS))?;
    handle.tcp_nodelay(true)?;
    handle.accept_encoding("")?;
    handle.useragent(constants::USER_AGENT)?;
    handle.url(url.as_ref())?;

    Ok(())
}

fn perform<T: Write>(handle: &Easy2<RequestHandler<T>>) -> Result<()> {
    let mut retries = 0;
    loop {
        match handle.perform() {
            Ok(()) => break,
            Err(_) if retries < constants::HTTP_RETRIES => {
                retries += 1;
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }

    handle.get_ref().check_error()?;

    let code = handle.response_code()?;
    if code != 200 {
        return Err(Error::Status(code, handle.effective_url()?.unwrap().to_owned()).into());
    }

    Ok(())
}

pub struct RawRequest<T>
where
    T: Write,
{
    handle: Easy2<RequestHandler<T>>,
}

impl<T: Write> RawRequest<T> {
    pub fn get(url: &Url, writer: T) -> Result<Self> {
        let mut request = Self {
            handle: Easy2::new(RequestHandler::new(writer)),
        };

        init_curl(&mut request.handle, url)?;
        request.handle.get(true)?;
        Ok(request)
    }

    pub fn url(&mut self, url: &Url) -> Result<()> {
        self.handle.url(url.as_ref())?;
        Ok(())
    }

    pub fn call(&mut self) -> Result<()> {
        perform(&self.handle)?;
        Ok(())
    }
}

pub struct TextRequest {
    handle: Easy2<RequestHandler<Vec<u8>>>,
}

impl TextRequest {
    pub fn get(url: &Url) -> Result<Self> {
        let mut request = Self::create();

        init_curl(&mut request.handle, url)?;
        request.handle.get(true)?;
        Ok(request)
    }

    pub fn post(url: &Url, data: &str) -> Result<Self> {
        let mut request = Self::create();

        init_curl(&mut request.handle, url)?;
        request.handle.post(true)?;
        request.handle.post_fields_copy(data.as_bytes())?;
        Ok(request)
    }

    pub fn header(&mut self, header: &str) -> Result<()> {
        let mut list = List::new();
        list.append(header)?;
        self.handle.http_headers(list)?;

        Ok(())
    }

    pub fn text(&mut self) -> Result<String> {
        self.handle.get_mut().writer.clear();

        perform(&self.handle)?;
        Ok(str::from_utf8(self.handle.get_ref().writer.as_slice())?.to_owned())
    }

    fn create() -> Self {
        Self {
            handle: Easy2::new(RequestHandler::new(Vec::new())),
        }
    }
}

struct RequestHandler<T>
where
    T: Write,
{
    pub writer: T,
    pub error: Option<io::Error>,
}

impl<T: Write> Handler for RequestHandler<T> {
    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        if let Err(e) = self.writer.write_all(data) {
            self.error = Some(e);
        }

        Ok(data.len())
    }

    fn debug(&mut self, kind: InfoType, data: &[u8]) {
        if matches!(kind, InfoType::Text) {
            let text = String::from_utf8_lossy(data);
            debug!("{}", text.strip_suffix('\n').unwrap_or(&text));
        }
    }
}

impl<T: Write> RequestHandler<T> {
    pub fn new(writer: T) -> Self {
        Self {
            writer,
            error: Option::default(),
        }
    }

    pub fn check_error(&self) -> Result<(), io::Error> {
        self.error
            .as_ref()
            .map_or_else(|| Ok(()), |e| Err(io::Error::from(e.kind())))
    }
}
