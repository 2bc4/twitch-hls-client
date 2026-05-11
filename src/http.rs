mod decoder;
mod request;
mod socks5;
mod url;

pub use request::{Request, TextRequest};
pub use url::{Scheme, Url};

use std::{
    fmt::{self, Display, Formatter},
    io::Write,
    sync::Arc,
};

use anyhow::Result;
use log::{debug, error};
use rustls::{ClientConfig, RootCertStore};

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

#[derive(Copy, Clone)]
pub enum Method {
    Get,
    Post,
    Head,
}

impl Display for Method {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::Get => f.write_str("GET"),
            Self::Post => f.write_str("POST"),
            Self::Head => f.write_str("HEAD"),
        }
    }
}

#[derive(Clone)]
pub struct Agent {
    tls_config: Arc<ClientConfig>,
}

impl Agent {
    pub fn new() -> Self {
        let mut roots = RootCertStore::empty();
        let res = rustls_native_certs::load_native_certs();

        for error in res.errors {
            error!("Failed to load certificates: {error}");
        }

        for cert in res.certs {
            if let Err(e) = roots.add(cert) {
                debug!("Invalid certificate: {e}");
            }
        }

        Self {
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
        let mut request = self.text();

        request
            .text_no_retry(Method::Head, url)
            .is_ok()
            .then_some(request)
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
