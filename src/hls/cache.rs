use std::{
    fs::{self, File, ReadDir},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Result};
use log::{debug, error};

use crate::http::{Agent, Connection, Url};

pub struct Cache {
    path: PathBuf,
}

impl Cache {
    const MAGIC: &str = concat!(env!("CARGO_PKG_NAME"), "\n");

    pub fn new(dir: &Option<String>, channel: &str, quality: &Option<String>) -> Option<Self> {
        let (dir, quality) = dir.as_ref().zip(quality.as_ref())?;

        match Self::read_dir(dir) {
            Ok(iter) => {
                for entry in iter {
                    let Ok(entry) = entry else {
                        continue;
                    };

                    Self::remove_if_stale(&entry.path());
                }
            }
            Err(e) => {
                error!("Failed to read playlist cache directory: {e}");
                return None;
            }
        }

        Some(Self {
            path: format!("{dir}/{channel}-{quality}").into(),
        })
    }

    pub fn get(&self, agent: &Agent) -> Option<Connection> {
        debug!("Trying playlist cache: {}", self.path.display());

        let mut file = Self::check_magic(&self.path)?;
        let mut string = String::new();
        file.read_to_string(&mut string).ok()?;

        let url = string.into();
        let Some(request) = agent.exists(&url) else {
            Self::remove_cache(&self.path);
            return None;
        };

        Some(Connection::new(url, request))
    }

    pub fn create(&self, url: &Url) {
        debug!("Creating playlist cache: {}", self.path.display());

        let file = File::create_new(&self.path);
        if let Err(e) = file.and_then(|mut f| write!(f, "{}{url}", Self::MAGIC)) {
            error!("Failed to create playlist cache: {e}");
        }
    }

    fn read_dir(dir: &str) -> Result<ReadDir> {
        let metadata = fs::metadata(dir)?;
        if !metadata.is_dir() || metadata.permissions().readonly() {
            bail!("Playlist cache path isn't a directory or is read only");
        }

        Ok(fs::read_dir(dir)?)
    }

    fn check_magic(path: &Path) -> Option<File> {
        let mut file = File::open(path).ok()?;
        let mut buf = [0u8; Self::MAGIC.len()];

        file.read_exact(&mut buf).ok()?;
        if buf != Self::MAGIC.as_bytes() {
            return None;
        }

        Some(file)
    }

    fn remove_cache(path: &Path) {
        debug!("Removing playlist cache: {}", path.display());
        if let Err(e) = fs::remove_file(path) {
            error!("Failed to remove playlist cache: {e}");
        }
    }

    fn remove_if_stale(path: &Path) -> Option<()> {
        const FOURTY_EIGHT_HOURS: Duration = Duration::from_secs(48 * 60 * 60);

        Self::check_magic(path)?;

        let metadata = fs::metadata(path).ok()?;
        let modified = metadata.modified().ok().and_then(|t| t.elapsed().ok())?;
        if metadata.is_file() && modified >= FOURTY_EIGHT_HOURS {
            Self::remove_cache(path);
        }

        Some(())
    }
}
