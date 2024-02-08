use std::{
    collections::hash_map::DefaultHasher,
    fmt::{self, Display, Formatter},
    hash::Hasher,
    io::{self, Write},
    mem,
    ops::Deref,
    str,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{ensure, Result};
use curl::easy::{Easy2, Handler, InfoType, IpResolve, List, WriteError};
use log::{debug, error, LevelFilter};

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

#[derive(Default, Clone, Debug)]
pub struct Url {
    hash: u64,
    inner: String,
}

impl From<&str> for Url {
    fn from(url: &str) -> Self {
        Self {
            hash: Self::hash(url),
            inner: url.to_owned(),
        }
    }
}

impl From<String> for Url {
    fn from(url: String) -> Self {
        Self {
            hash: Self::hash(&url),
            inner: url,
        }
    }
}

impl PartialEq for Url {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl Deref for Url {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Display for Url {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Url {
    fn hash(url: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        hasher.write(url.as_bytes());

        hasher.finish()
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
        parser.parse_fn(&mut self.timeout, "--http-timeout", |arg| {
            Ok(Duration::try_from_secs_f64(arg.parse()?)?)
        })?;
        parser.parse(&mut self.user_agent, "--user-agent")?;

        Ok(())
    }
}

#[derive(Clone)]
pub struct Agent {
    args: Arc<Args>,
    certs: Arc<Mutex<Vec<u8>>>,
}

impl Agent {
    pub fn new(args: &Args) -> Result<Self> {
        Ok(Self {
            args: Arc::new(args.to_owned()),
            certs: Arc::new(Mutex::new(rustls_native_certs::load_native_certs()?)),
        })
    }

    pub fn get(&self, url: &Url) -> Result<TextRequest> {
        TextRequest::get(Request::new(StringWriter::default(), url, self.clone())?)
    }

    pub fn post(&self, url: &Url, data: &str) -> Result<TextRequest> {
        TextRequest::post(
            Request::new(StringWriter::default(), url, self.clone())?,
            data,
        )
    }

    pub fn writer<T: Write>(&self, writer: T, url: &Url) -> Result<WriterRequest<T>> {
        let request = WriterRequest::new(Request::new(writer, url, self.clone())?)?;

        //Currently this is the last time certs are used so they can be freed here
        let mut certs = self
            .certs
            .lock()
            .expect("Failed to lock certs mutex while freeing");

        *certs = Vec::default();
        Ok(request)
    }
}

pub struct TextRequest {
    request: Request<StringWriter>,
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
        Ok(mem::take(&mut self.request.get_mut().0))
    }

    pub fn encode(&mut self, data: &str) -> String {
        self.request.handle.url_encode(data.as_bytes())
    }

    fn get(mut request: Request<StringWriter>) -> Result<Self> {
        request.handle.get(true)?;
        Ok(Self { request })
    }

    fn post(mut request: Request<StringWriter>, data: &str) -> Result<Self> {
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

        request
            .handle
            .ssl_cainfo_blob(&agent.certs.lock().expect("Failed to lock certs mutex"))?;

        if request.args.force_ipv4 {
            request.handle.ip_resolve(IpResolve::V4)?;
        }

        request.handle.timeout(request.args.timeout)?;
        request.handle.tcp_nodelay(true)?;
        request.handle.accept_encoding("")?; //empty string accepts all available encodings
        request.handle.buffer_size(constants::CURL_BUFFER_SIZE)?;
        request.handle.useragent(&request.args.user_agent)?;
        request.url(url)?;
        Ok(request)
    }

    fn get_mut(&mut self) -> &mut T {
        &mut self.handle.get_mut().writer
    }

    fn perform(&mut self) -> Result<()> {
        let mut retries = 0;
        loop {
            match self.handle.perform() {
                Ok(()) => break,
                Err(e) if self.handle.get_ref().error.is_some() => {
                    let io_error = self.handle.get_mut().error.take().ok_or(e)?;
                    return Err(io_error.into());
                }
                Err(e) if retries < self.args.retries => {
                    error!("http: {e}");
                    retries += 1;
                }
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
                url.starts_with("https"),
                "URL protocol is not HTTPS and --force-https is enabled: {url}"
            );
        }

        self.handle.url(url)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_https() {
        let agent = Agent::new(&Args {
            force_https: true,
            ..Default::default()
        })
        .unwrap();

        assert!(agent.get(&"http://not-https.invalid".into()).is_err());
        assert!(agent.get(&"https://is-https.invalid".into()).is_ok());
    }
}
