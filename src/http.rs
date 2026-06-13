mod decoder;
mod proxy;
mod request;
mod url;

pub use request::{Request, TextRequest};
pub use url::{Scheme, Url};

use std::fmt::{self, Display, Formatter};

use anyhow::{Context, Result};

const MAX_HEADERS_SIZE: usize = 4 * 1024;

#[derive(Debug)]
pub struct StatusError(u16, Url);

impl std::error::Error for StatusError {}

impl Display for StatusError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "HTTP request failed with status {}: {}", self.0, self.1)
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

fn parse_status(headers: &str) -> Result<u16> {
    headers
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .context("Failed to parse HTTP status code")
}
