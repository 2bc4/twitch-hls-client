mod decoder;
mod request;
mod url;

pub use request::{TextRequest, WriterRequest};
pub use url::Url;

use std::{
    fmt::{self, Display, Formatter},
    io::Write,
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
use request::{Method, Request, StringWriter};

#[derive(Debug)]
pub enum Error {
    Status(u16, Url),
    NotFound(Url),
}

impl std::error::Error for Error {}

impl Display for Error {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::Status(code, url) => write!(f, "Status code {code} on {url}"),
            Self::NotFound(url) => write!(f, "Not found: {url}"),
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
    pub fn new(args: &Args) -> Self {
        let mut roots = RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().unwrap_or_default() {
            //Ignore parsing errors, OS can have broken certs.
            if let Err(e) = roots.add(cert) {
                debug!("Invalid certificate: {e}");
            }
        }

        Self {
            args: Arc::new(args.to_owned()),
            tls_config: Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(Arc::new(roots))
                    .with_no_client_auth(),
            ),
        }
    }

    pub fn get(&self, url: Url) -> Result<TextRequest> {
        Ok(TextRequest::new(Request::new(
            StringWriter::default(),
            Method::Get,
            url,
            String::default(),
            self.clone(),
        )?))
    }

    pub fn post(&self, url: Url, data: String) -> Result<TextRequest> {
        Ok(TextRequest::new(Request::new(
            StringWriter::default(),
            Method::Post,
            url,
            data,
            self.clone(),
        )?))
    }

    pub fn writer<T: Write>(&self, writer: T, url: Url) -> Result<WriterRequest<T>> {
        WriterRequest::new(Request::new(
            writer,
            Method::Get,
            url,
            String::default(),
            self.clone(),
        )?)
    }
}
