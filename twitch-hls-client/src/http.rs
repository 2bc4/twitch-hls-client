use std::{
    fmt,
    io::{self, Write},
    str,
    sync::Arc,
    time::Duration,
};

use anyhow::{ensure, Context, Result};
use curl::easy::{Easy2, Handler, InfoType, IpResolve, List, WriteError};
use log::{debug, LevelFilter};
use url::Url;

use crate::{
    args::{ArgParse, Parser},
    constants,
};

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

#[derive(Debug, Clone)]
pub struct Args {
    pub force_https: bool,
    pub force_ipv4: bool,
    pub retries: u64,
    pub timeout: Duration,
    pub user_agent: String,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            retries: 3,
            timeout: Duration::from_secs(10),
            user_agent: constants::USER_AGENT.to_owned(),
            force_https: bool::default(),
            force_ipv4: bool::default(),
        }
    }
}

impl ArgParse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_switch(&mut self.force_https, "--force-https")?;
        parser.parse_switch(&mut self.force_ipv4, "--force-ipv4")?;
        parser.parse(&mut self.retries, "--http-retries")?;
        parser.parse_fn(&mut self.timeout, "--http-timeout", Self::parse_duration)?;
        parser.parse(&mut self.user_agent, "--user-agent")?;

        Ok(())
    }
}

impl Args {
    fn parse_duration(arg: &str) -> Result<Duration> {
        Ok(Duration::try_from_secs_f64(arg.parse()?)?)
    }
}

#[derive(Clone)]
pub struct Agent {
    args: Arc<Args>,
    certs: Arc<Vec<u8>>,
}

impl Agent {
    pub fn new(args: &Args) -> Result<Self> {
        Ok(Self {
            args: Arc::new(args.to_owned()),
            certs: Arc::new(rustls_native_certs::load_native_certs()?),
        })
    }

    pub fn get(&self, url: &Url) -> Result<TextRequest> {
        TextRequest::get(Request::new(Vec::new(), url, self.clone())?)
    }

    pub fn post(&self, url: &Url, data: &str) -> Result<TextRequest> {
        TextRequest::post(Request::new(Vec::new(), url, self.clone())?, data)
    }

    pub fn writer<T: Write>(&self, writer: T, url: &Url) -> Result<WriterRequest<T>> {
        WriterRequest::new(Request::new(writer, url, self.clone())?)
    }
}

pub struct TextRequest {
    request: Request<Vec<u8>>,
}

impl TextRequest {
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

    pub fn url(&mut self) -> Result<Url> {
        Ok(self
            .request
            .handle
            .effective_url()?
            .context("Failed to get URL from request")?
            .parse()?)
    }

    fn get(mut request: Request<Vec<u8>>) -> Result<Self> {
        request.handle.get(true)?;
        Ok(Self { request })
    }

    fn post(mut request: Request<Vec<u8>>, data: &str) -> Result<Self> {
        request.handle.post(true)?;
        request.handle.post_fields_copy(data.as_bytes())?;

        Ok(Self { request })
    }
}

pub struct WriterRequest<T>
where
    T: Write,
{
    request: Request<T>,
}

impl<T: Write> WriterRequest<T> {
    pub fn call(&mut self, url: &Url) -> Result<()> {
        self.request.url(url)?;
        self.request.perform()
    }

    fn new(mut request: Request<T>) -> Result<Self> {
        request.handle.get(true)?;
        request.perform()?;

        Ok(Self { request })
    }
}

struct Request<T>
where
    T: Write,
{
    handle: Easy2<RequestHandler<T>>,
    args: Arc<Args>,
}

impl<T: Write> Request<T> {
    fn new(writer: T, url: &Url, agent: Agent) -> Result<Self> {
        let mut request = Self {
            handle: Easy2::new(RequestHandler {
                writer,
                error: Option::default(),
            }),
            args: agent.args,
        };

        request
            .handle
            .verbose(log::max_level() == LevelFilter::Debug)?;

        if request.args.force_ipv4 {
            request.handle.ip_resolve(IpResolve::V4)?;
        }

        request.handle.ssl_cainfo_blob(&agent.certs)?;
        request.handle.timeout(request.args.timeout)?;
        request.handle.tcp_nodelay(true)?;
        request.handle.accept_encoding("")?; //empty string accepts all available encodings
        request.handle.useragent(&request.args.user_agent)?;
        request.url(url)?;
        Ok(request)
    }

    fn get_ref(&self) -> &T {
        &self.handle.get_ref().writer
    }

    fn get_mut(&mut self) -> &mut T {
        &mut self.handle.get_mut().writer
    }

    fn perform(&mut self) -> Result<()> {
        let mut retries = 0;
        loop {
            match self.handle.perform() {
                Ok(()) => break,
                Err(e) if e.is_write_error() => {
                    let io_error = self.handle.get_mut().error.take().ok_or(e)?;
                    return Err(io_error.into());
                }
                Err(_) if retries < self.args.retries => retries += 1,
                Err(e) => return Err(e.into()),
            }
        }

        self.get_mut().flush()?; //signal that the request is done

        let code = self.handle.response_code()?;
        if code == 200 {
            Ok(())
        } else {
            let url = self
                .handle
                .effective_url()?
                .unwrap_or("<unknown>")
                .to_owned();

            if code == 404 {
                return Err(Error::NotFound(url).into());
            }

            Err(Error::Status(code, url).into())
        }
    }

    fn url(&mut self, url: &Url) -> Result<()> {
        if self.args.force_https {
            ensure!(
                url.scheme() == "https",
                "URL protocol is not HTTPS and --force-https is enabled: {url}"
            );
        }

        self.handle.url(url.as_ref())?;
        Ok(())
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
            return Ok(0);
        }

        Ok(data.len())
    }

    fn debug(&mut self, kind: InfoType, data: &[u8]) {
        if matches!(kind, InfoType::Text) {
            let text = String::from_utf8_lossy(data);
            if text.starts_with("Found bundle") || text.starts_with("Can not multiplex") {
                return;
            }

            debug!("{}", text.strip_suffix('\n').unwrap_or(&text));
        }
    }
}
