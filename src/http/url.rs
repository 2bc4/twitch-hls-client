use std::{
    fmt::{self, Display, Formatter},
    ops::{Deref, DerefMut},
};

use anyhow::{bail, Context, Result};

#[derive(Default, Clone, Debug)]
pub struct Url {
    pub scheme: Scheme,
    inner: String,
}

impl From<&str> for Url {
    fn from(inner: &str) -> Self {
        Self {
            scheme: Scheme::new(inner),
            inner: inner.to_owned(),
        }
    }
}

impl From<String> for Url {
    fn from(inner: String) -> Self {
        Self {
            scheme: Scheme::new(&inner),
            inner,
        }
    }
}

impl Deref for Url {
    type Target = String;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Url {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Display for Url {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str(&self.inner)
    }
}

impl Url {
    pub fn host(&self) -> Result<&str> {
        let host = self
            .inner
            .split_terminator('/')
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
        if let Some(port) = self
            .inner
            .split_terminator('/')
            .nth(2)
            .and_then(|p| p.split_once(':'))
        {
            return port.1.parse().context("Failed to parse port in URL");
        }

        match self.scheme {
            Scheme::Http => Ok(80),
            Scheme::Https => Ok(443),
            Scheme::Unknown => bail!("Unknown scheme in URL"),
        }
    }
}

#[derive(Default, Copy, Clone, Debug, PartialEq, Eq)]
pub enum Scheme {
    Http,
    Https,

    #[default]
    Unknown,
}

impl Display for Scheme {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http => f.write_str("http"),
            Self::Https => f.write_str("https"),
            Self::Unknown => f.write_str("<unknown>"),
        }
    }
}

impl Scheme {
    fn new(url: &str) -> Self {
        match url.split(':').next() {
            Some("http") => Self::Http,
            Some("https") => Self::Https,
            _ => Self::Unknown,
        }
    }
}
