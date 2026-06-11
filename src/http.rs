mod decoder;
mod request;
mod socks5;
mod url;

pub use request::{Request, TextRequest};
pub use url::{Scheme, Url};

use std::fmt::{self, Display, Formatter};

use anyhow::Result;

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

//Helper for passing around a url with a text request
pub struct Connection {
    pub url: Url,
    pub request: TextRequest,
}

impl Connection {
    pub fn new(url: Url) -> Self {
        Self {
            url,
            request: TextRequest::new(),
        }
    }

    pub const fn new_with_request(url: Url, request: TextRequest) -> Self {
        Self { url, request }
    }

    pub fn text(&mut self) -> Result<&str> {
        self.request.text(Method::Get, &self.url)
    }
}
