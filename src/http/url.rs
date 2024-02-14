use std::{
    fmt::{self, Display, Formatter},
    hash::{DefaultHasher, Hasher},
    ops::Deref,
};

use anyhow::{bail, Context, Result};

use crate::logger;

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
    pub fn scheme(&self) -> Result<&str> {
        self.inner
            .split(':')
            .next()
            .context("Failed to parse scheme in URL")
    }

    pub fn host(&self) -> Result<&str> {
        let host = self
            .inner
            .split('/')
            .nth(2)
            .context("Failed to parse host in URL")?;

        if let Some(split) = host.split_once(':') {
            Ok(split.0)
        } else {
            Ok(host)
        }
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

        match self.scheme()? {
            "http" => Ok(80),
            "https" => Ok(443),
            _ => bail!("Unsupported scheme in URL"),
        }
    }

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
