mod cache;
mod multivariant;
mod playlist;
mod segment;

pub use multivariant::Stream;
pub use playlist::Playlist;
pub use segment::{Handler, ResetError};

use std::fmt::{self, Display, Formatter};

use crate::http::StatusError;
use anyhow::{Result, bail};

#[derive(Debug)]
pub struct OfflineError;

impl std::error::Error for OfflineError {}

impl Display for OfflineError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str("Stream is offline or unavailable")
    }
}

#[derive(Debug, Default, Clone)]
pub enum Passthrough {
    Variant,
    Multivariant,

    #[default]
    Disabled,
}

impl Passthrough {
    pub fn new(arg: &str) -> Result<Self> {
        match arg {
            "variant" => Ok(Self::Variant),
            "multivariant" => Ok(Self::Multivariant),
            "disabled" => Ok(Self::Disabled),
            _ => bail!("Invalid passthrough mode"),
        }
    }
}

fn map_if_offline(error: anyhow::Error) -> anyhow::Error {
    if StatusError::is_not_found(&error) {
        return OfflineError.into();
    }

    error
}
