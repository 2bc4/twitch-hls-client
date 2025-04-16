mod decoder;
mod request;
mod url;

pub use request::{Request, TextRequest};
pub use url::{Scheme, Url};

use std::{
    borrow::Cow,
    fmt::{self, Display, Formatter},
    io::{self, Write},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use log::{debug, error};
use rustls::{ClientConfig, RootCertStore};

use crate::{
    args::{Parse, Parser},
    constants,
};

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
            .downcast_ref::<Self>()
            .is_some_and(|Self(code, _)| *code == 404)
    }
}

#[derive(Debug, Clone)]
pub struct Args {
    force_https: bool,
    force_ipv4: bool,
    retries: u64,
    timeout: Duration,
    user_agent: Cow<'static, str>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            retries: 3,
            timeout: Duration::from_secs(10),
            user_agent: constants::USER_AGENT.into(),
            force_https: bool::default(),
            force_ipv4: bool::default(),
        }
    }
}

impl Parse for Args {
    fn parse(&mut self, parser: &mut Parser) -> Result<()> {
        parser.parse_switch(&mut self.force_https, "--force-https")?;
        parser.parse_switch(&mut self.force_ipv4, "--force-ipv4")?;
        parser.parse(&mut self.retries, "--http-retries")?;
        parser.parse_duration(&mut self.timeout, "--http-timeout")?;
        parser.parse_cow_string(&mut self.user_agent, "--user-agent")?;

        Ok(())
    }
}

#[derive(Copy, Clone)]
pub enum Method {
    Get,
    Post,
}

impl Display for Method {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::Get => f.write_str("GET"),
            Self::Post => f.write_str("POST"),
        }
    }
}

#[derive(Clone)]
pub struct Agent {
    args: Arc<Args>,
    tls_config: Arc<ClientConfig>,
}

impl Agent {
    pub fn new(args: Args) -> Self {
        let mut roots = RootCertStore::empty();
        let res = rustls_native_certs::load_native_certs();

        for error in res.errors {
            error!("Failed to load certificates: {error}");
        }

        for cert in res.certs {
            //Ignore parsing errors, OS can have broken certs
            if let Err(e) = roots.add(cert) {
                debug!("Invalid certificate: {e}");
            }
        }

        Self {
            args: Arc::new(args),
            tls_config: Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(Arc::new(roots))
                    .with_no_client_auth(),
            ),
        }
    }

    pub fn text(&self) -> TextRequest {
        TextRequest::new(self.clone())
    }

    pub fn binary<W: Write>(&self, writer: W) -> Request<W> {
        Request::new(writer, self.clone())
    }

    pub fn exists(&self, url: &Url) -> Option<TextRequest> {
        let mut request = self.binary(io::sink());

        request
            .call(Method::Get, url)
            .is_ok()
            .then(|| request.into_text_request())
    }
}

//Helper for passing around a url with a text request
pub struct Connection {
    pub url: Url,
    pub request: TextRequest,
}

impl Connection {
    pub const fn new(url: Url, request: TextRequest) -> Self {
        Self { url, request }
    }

    pub fn text(&mut self) -> Result<&str> {
        self.request.text(Method::Get, &self.url)
    }
}
