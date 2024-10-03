use std::{fs, time::Duration};

use log::{debug, error};

use crate::http::{Agent, Url};

pub struct Cache {
    path: String,
}

impl Cache {
    pub fn new(dir: Option<String>, channel: &str, quality: &Option<String>) -> Option<Self> {
        if let Some(dir) = dir {
            if let Some(quality) = quality {
                match fs::metadata(&dir) {
                    Ok(metadata) if metadata.is_dir() && !metadata.permissions().readonly() => {
                        Self::remove_stale(&dir);

                        return Some(Self {
                            path: format!("{dir}/{channel}-{quality}"),
                        });
                    }
                    Err(e) => error!("Failed to open playlist cache directory: {e}"),
                    _ => error!("Playlist cache path is not writable or is not a directory"),
                }
            }
        }

        None
    }

    pub fn get(&self, agent: &Agent) -> Option<Url> {
        debug!("Reading playlist cache: {}", self.path);

        let url: Url = fs::read_to_string(&self.path).ok()?.trim_end().into();
        if !agent.exists(url.clone()) {
            debug!("Removing playlist cache: {}", self.path);
            if let Err(e) = fs::remove_file(&self.path) {
                error!("Failed to remove playlist cache: {e}");
            }

            return None;
        }

        Some(url)
    }

    pub fn create(&self, url: &Url) {
        debug!("Creating playlist cache: {}", self.path);
        if let Err(e) = fs::write(&self.path, url.as_str()) {
            error!("Failed to create playlist cache: {e}");
        }
    }

    fn remove_stale(dir: &str) {
        let iter = match fs::read_dir(dir) {
            Ok(iter) => iter,
            Err(e) => {
                error!("Failed to read playlist cache directory: {e}");
                return;
            }
        };

        for entry in iter {
            let Ok(entry) = entry else {
                continue;
            };

            if let Some(duration) = fs::metadata(entry.path())
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
            {
                //After 48 hours a playlist cannot be valid
                if duration >= Duration::from_secs(48 * 60 * 60) {
                    debug!("Removing stale playlist cache: {}", entry.path().display());
                    if let Err(e) = fs::remove_file(entry.path()) {
                        error!("Failed to remove stale playlist cache: {e}");
                    }
                }
            }
        }
    }
}
