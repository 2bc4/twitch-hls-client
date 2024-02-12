use std::{
    fmt::{self, Display, Formatter},
    hash::{DefaultHasher, Hasher},
    io::{self, Write},
    ops::Deref,
    str,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{ensure, Result};
use curl::easy::{Easy, Easy2, Handler, InfoType, IpResolve, List, WriteError};
use log::{debug, error, info};

use crate::{
    args::{ArgParse, Parser},
    constants, logger,
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
    #[allow(dead_code)] //used for debug logging
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
        self.inner == other.inner
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
        if logger::is_debug() {
            let mut hasher = DefaultHasher::new();
            hasher.write(url.as_bytes());

            hasher.finish()
        } else {
            u64::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct Args {
    force_https: bool,
    force_ipv4: bool,
    retries: u64,
    timeout: Duration,
    user_agent: String,
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
        TextRequest::get(Request::new(
            StringWriter::default(),
            url,
            false,
            self.clone(),
        )?)
    }

    pub fn post(&self, url: &Url, data: &str) -> Result<TextRequest> {
        TextRequest::post(
            Request::new(StringWriter::default(), url, false, self.clone())?,
            data,
        )
    }

    pub fn writer<T: Write>(&self, writer: T, url: &Url) -> Result<WriterRequest<T>> {
        let request = WriterRequest::new(Request::new(writer, url, true, self.clone())?)?;

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

    pub fn text(&mut self) -> Result<&str> {
        self.request.get_mut().0.clear();
        self.request.perform()?;

        Ok(&self.request.get_mut().0)
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

pub fn url_encode(text: &str) -> String {
    //Why is this tied to a handle??
    Easy::new().url_encode(text.as_bytes())
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
    should_resume: bool,
}

impl<T: Write> Request<T> {
    fn new(writer: T, url: &Url, should_resume: bool, agent: Agent) -> Result<Self> {
        let mut request = Self {
            handle: Easy2::new(RequestHandler {
                writer,
                error: Option::default(),
                written: usize::default(),
            }),
            args: agent.args,
            should_resume,
        };

        request.handle.verbose(logger::is_debug())?;
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
                    retries += 1;
                    error!("http: {e}");

                    if self.should_resume {
                        if e.is_range_error() || e.is_recv_error() {
                            self.handle.resume_from(0)?;
                            self.should_resume = false;

                            continue;
                        }

                        let written = self.handle.get_ref().written;
                        if written > 0 {
                            info!("Resuming from offset: {written} bytes");
                            self.handle.resume_from(written as u64)?;
                        }
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }

        self.get_mut().flush()?; //signal that the request is done
        self.handle.get_mut().written = 0;
        self.handle.resume_from(0)?;

        let code = self.handle.response_code()?;
        if code == 200 || code == 206 {
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
                url.starts_with("https://"),
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
    written: usize,
}

impl<T: Write> Handler for RequestHandler<T> {
    fn write(&mut self, data: &[u8]) -> Result<usize, WriteError> {
        if let Err(e) = self.writer.write_all(data) {
            self.error = Some(e);
            return Ok(0);
        }

        self.written += data.len();
        Ok(data.len())
    }

    fn debug(&mut self, kind: InfoType, data: &[u8]) {
        if matches!(kind, InfoType::Text) {
            let text = String::from_utf8_lossy(data);
            if text.starts_with("Found bundle")
                || text.starts_with("Can not multiplex")
                || text.starts_with("Re-using")
                || text.starts_with("Leftovers")
                || text.ends_with("left intact\n")
            {
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
