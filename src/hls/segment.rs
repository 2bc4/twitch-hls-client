use std::{mem, str::FromStr, thread, time::Duration as StdDuration, time::Instant};

use anyhow::{Context, Result};
use log::{debug, info};

use super::playlist::MediaPlaylist;
use crate::worker::Worker;

//Used for av1/hevc streams
pub struct Header(pub Option<String>);

impl FromStr for Header {
    type Err = anyhow::Error;

    fn from_str(playlist: &str) -> Result<Self, Self::Err> {
        let header_url = playlist
            .lines()
            .find(|s| s.starts_with("#EXT-X-MAP"))
            .and_then(|s| s.split_once('='))
            .map(|s| s.1.replace('"', ""));

        if let Some(header_url) = header_url {
            return Ok(Self(Some(header_url)));
        }

        Ok(Self(None))
    }
}

#[derive(Default, Clone, PartialEq, Debug)]
pub struct Duration {
    pub is_ad: bool,
    duration: StdDuration,
}

impl FromStr for Duration {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        //can't wait too long or the server will close the socket
        const MAX_DURATION: StdDuration = StdDuration::from_secs(3);
        let duration = StdDuration::try_from_secs_f32(
            s.split_once(':')
                .and_then(|s| s.1.split_once(','))
                .map(|s| s.0.parse())
                .context("Invalid segment duration")??,
        )
        .context("Failed to parse segment duration")?;

        Ok(Self {
            duration: if duration < MAX_DURATION {
                duration
            } else {
                MAX_DURATION
            },
            is_ad: s.contains('|'),
        })
    }
}

impl Duration {
    pub fn sleep(&self, elapsed: StdDuration) {
        Self::sleep_thread(self.duration, elapsed);
    }

    pub fn sleep_half(&self, elapsed: StdDuration) {
        if let Some(half) = self.duration.checked_div(2) {
            Self::sleep_thread(half, elapsed);
        }
    }

    fn sleep_thread(duration: StdDuration, elapsed: StdDuration) {
        if let Some(sleep_time) = duration.checked_sub(elapsed) {
            debug!("Sleeping thread for {:?}", sleep_time);
            thread::sleep(sleep_time);
        }
    }
}

#[derive(Default, Clone, Debug)]
pub enum Segment {
    Normal(Duration, String),
    NextPrefetch(Duration, String),
    NewestPrefetch(String),

    #[default]
    Unknown,
}

impl PartialEq for Segment {
    fn eq(&self, other: &Self) -> bool {
        let (_, self_url) = self.destructure_ref();
        let (_, other_url) = other.destructure_ref();

        self_url == other_url
    }
}

impl Segment {
    pub fn find_next(&self, segments: &mut [Segment]) -> Option<Segment> {
        debug!("Previous: {self:?}\n");
        if let Some(idx) = segments.iter().position(|s| self == s) {
            let idx = idx + 1;
            if idx == segments.len() {
                return None;
            }

            //shouldn't panic, already did bounds check
            return Some(mem::take(&mut segments[idx]));
        }

        Some(Segment::Unknown)
    }

    pub fn destructure(self) -> (Option<Duration>, String) {
        match self {
            Self::Normal(duration, url) | Self::NextPrefetch(duration, url) => {
                (Some(duration), url)
            }
            Self::NewestPrefetch(url) => (None, url),
            Self::Unknown => (None, String::default()),
        }
    }

    pub fn destructure_ref(&self) -> (Option<&Duration>, Option<&String>) {
        match self {
            Self::Normal(duration, url) | Self::NextPrefetch(duration, url) => {
                (Some(duration), Some(url))
            }
            Self::NewestPrefetch(url) => (None, Some(url)),
            Self::Unknown => (None, None),
        }
    }
}

pub struct Handler {
    playlist: MediaPlaylist,
    worker: Worker,
    prev_segment: Segment,
    init: bool,
}

impl Handler {
    pub fn new(playlist: MediaPlaylist, worker: Worker) -> Self {
        Self {
            playlist,
            worker,
            prev_segment: Segment::default(),
            init: true,
        }
    }

    pub fn reload(&mut self) -> Result<()> {
        self.playlist.reload()
    }

    pub fn process(&mut self, time: Instant) -> Result<()> {
        let mut segments = self.playlist.segments()?;
        let duration = segments.iter().rev().find_map(|s| match s {
            Segment::Normal(duration, _) => Some(duration),
            _ => None,
        });

        if let Some(duration) = duration {
            if duration.is_ad {
                info!("Filtering ad segment...");
                duration.sleep(time.elapsed());

                return Ok(());
            }
        }

        if let Some(segment) = self.prev_segment.find_next(segments.as_mut_slice()) {
            self.prev_segment = segment.clone();
            match segment {
                Segment::Normal(duration, url) | Segment::NextPrefetch(duration, url) => {
                    self.worker.url(url)?;
                    duration.sleep(time.elapsed());
                }
                Segment::NewestPrefetch(url) => self.worker.sync_url(url)?,
                Segment::Unknown => {
                    if self.init {
                        self.init = false;
                    } else {
                        info!("Failed to find next segment, jumping to newest...");
                    }

                    let segment = segments
                        .into_iter()
                        .last()
                        .context("Failed to find newest segment")?;

                    self.prev_segment = segment.clone();
                    let (_, url) = segment.destructure();

                    self.worker.sync_url(url)?;
                }
            }
        } else {
            info!("Playlist unchanged, retrying...");
            let (duration, _) = self.prev_segment.destructure_ref();
            duration
                .context("Failed to get segment duration while retrying")?
                .sleep_half(time.elapsed());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::playlist::tests::create_playlist;
    use super::super::tests::PLAYLIST;
    use super::*;

    #[test]
    fn parse_header() {
        assert_eq!(
            PLAYLIST.parse::<Header>().unwrap().0,
            Some("http://header.invalid".to_string()),
        );
    }

    #[test]
    fn parse_prefetch_segments() {
        let playlist = create_playlist();

        let segments = playlist.segments().unwrap();
        assert_eq!(
            segments.into_iter().last().unwrap(),
            Segment::NewestPrefetch("http://newest-prefetch-url.invalid".to_string()),
        );

        let segments = playlist.segments().unwrap();
        assert_eq!(
            segments[segments.len() - 2],
            Segment::NextPrefetch(
                Duration {
                    duration: StdDuration::from_secs_f32(0.978),
                    is_ad: false,
                },
                "http://next-prefetch-url.invalid".to_string(),
            ),
        );
    }
}
