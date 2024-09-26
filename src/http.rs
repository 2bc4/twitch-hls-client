mod decoder;
mod request;
mod url;

pub use request::{Request, TextRequest};
pub use url::{Scheme, Url};

use std::{
    fmt::{self, Display, Formatter},
    io::{self, Write},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use log::debug;
use rustls::{ClientConfig, RootCertStore};

use crate::{
    args::{ArgParser, Parser},
    constants,
};
use request::Method;

#[derive(Debug)]
pub struct StatusError(u16, Url);

impl std::error::Error for StatusError {}

impl Display for StatusError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Status code {} on {}", self.0, self.1)
    }
}

impl StatusError {
    pub fn is_not_found(error: &anyhow::Error) -> bool {
        error
            .downcast_ref::<StatusError>()
            .is_some_and(|StatusError(code, _)| *code == 404)
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

impl ArgParser for Args {
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
    tls_config: Arc<ClientConfig>,
}

impl Agent {
    pub fn new(args: Args) -> Result<Self> {
        let mut roots = RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs()? {
            //Ignore parsing errors, OS can have broken certs.
            if let Err(e) = roots.add(cert) {
                debug!("Invalid certificate: {e}");
            }
        }

        Ok(Self {
            args: Arc::new(args),
            tls_config: Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(Arc::new(roots))
                    .with_no_client_auth(),
            ),
        })
    }

    pub fn get(&self, url: Url) -> Result<TextRequest> {
        TextRequest::new(Method::Get, url, String::default(), self.clone())
    }

    pub fn post(&self, url: Url, data: String) -> Result<TextRequest> {
        TextRequest::new(Method::Post, url, data, self.clone())
    }

    pub fn exists(&self, url: Url) -> bool {
        let Ok(mut request) = self.request(io::sink(), url) else {
            return false;
        };

        request.call().is_ok()
    }

    pub fn request<T: Write>(&self, writer: T, url: Url) -> Result<Request<T>> {
        Request::new(writer, Method::Get, url, String::default(), self.clone())
    }
}
