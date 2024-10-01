use std::{
    fmt::{self, Display, Formatter},
    ops::Deref,
};

use anyhow::{bail, Context, Result};

#[derive(Default, Clone, Debug)]
pub struct Url {
    #[allow(dead_code)] //used for debug logging
    hash: u64,

    pub scheme: Scheme,
    inner: String,
}

impl From<&str> for Url {
    fn from(url: &str) -> Self {
        Self {
            hash: Self::hash(url),
            scheme: Scheme::new(url),
            inner: url.to_owned(),
        }
    }
}

impl From<String> for Url {
    fn from(url: String) -> Self {
        Self {
            hash: Self::hash(&url),
            scheme: Scheme::new(&url),
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
    pub fn host(&self) -> Result<&str> {
        let host = self
            .inner
            .split('/')
            .nth(2)
            .context("Failed to parse host in URL")?;

        Ok(host.split_once(':').map_or(host, |(s, _)| s))
    }

    pub fn path(&self) -> Result<&str> {
        self.inner
            .splitn(4, '/')
            .nth(3)
            .context("Failed to parse path in URL")
    }

    pub fn port(&self) -> Result<u16> {
        if let Some(port) = self.inner.split('/').nth(2).and_then(|p| p.split_once(':')) {
            return port.1.parse().context("Failed to parse port in URL");
        }

        match self.scheme {
            Scheme::Http => Ok(80),
            Scheme::Https => Ok(443),
            Scheme::Unknown => bail!("Unknown scheme in URL"),
        }
    }

    #[cfg(feature = "debug-logging")]
    fn hash(url: &str) -> u64 {
        use crate::logger;
        use std::hash::{DefaultHasher, Hasher};

        if logger::is_debug() {
            let mut hasher = DefaultHasher::new();
            hasher.write(url.as_bytes());

            hasher.finish()
        } else {
            u64::default()
        }
    }

    #[cfg(not(feature = "debug-logging"))]
    fn hash(_url: &str) -> u64 {
        u64::default()
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,

    #[default]
    Unknown,
}

impl Scheme {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Unknown => "<unknown>",
        }
    }

    fn new(url: &str) -> Self {
        match url.split(':').next() {
            Some("http") => Self::Http,
            Some("https") => Self::Https,
            _ => Self::Unknown,
        }
    }
}
