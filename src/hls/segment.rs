use std::{cmp::Ordering, mem, str::FromStr, thread, time::Duration as StdDuration, time::Instant};

use anyhow::{Context, Result};
use log::{debug, info};

use super::{MediaPlaylist, media_playlist::QueueRange};
use crate::{http::Url, worker::Worker};

#[derive(Default, Copy, Clone, Debug)]
pub struct Duration {
    is_ad: bool,
    inner: StdDuration,
}

impl FromStr for Duration {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            is_ad: s.contains('|'),
            inner: StdDuration::try_from_secs_f32(
                s.split_once(',')
                    .map(|s| s.0.parse())
                    .context("Invalid segment duration")??,
            )
            .context("Failed to parse segment duration")?,
        })
    }
}

impl PartialEq for Duration {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}

impl PartialOrd for Duration {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.inner.cmp(&other.inner))
    }
}

impl Duration {
    //can't wait too long or the server will close the socket
    const MAX: Self = Self {
        is_ad: false,
        inner: StdDuration::from_secs(3),
    };

    pub fn sleep(&self, elapsed: StdDuration) {
        if self.inner >= Self::MAX.inner {
            self.sleep_half(elapsed);
            return;
        }

        Self::sleep_thread(self.inner, elapsed);
    }

    pub fn sleep_half(&self, elapsed: StdDuration) {
        if let Some(half) = self.inner.checked_div(2) {
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

#[derive(Debug)]
pub enum Segment {
    Normal(Duration, Url),
    Prefetch(Url),
}

pub struct Handler {
    worker: Worker,
    init: bool,
}

impl Handler {
    pub const fn new(worker: Worker) -> Self {
        Self { worker, init: true }
    }

    pub fn process(&mut self, playlist: &mut MediaPlaylist, time: Instant) -> Result<()> {
        let last_duration = playlist
            .last_duration()
            .context("Failed to find last segment duration")?;

        if last_duration.is_ad {
            info!("Filtering ad segment...");
            last_duration.sleep(time.elapsed());

            return Ok(());
        }

        match playlist.segments() {
            QueueRange::Partial(ref mut segments) => {
                for segment in segments {
                    debug!("Sending segment to worker:\n{segment:?}");
                    match segment {
                        Segment::Normal(_, url) | Segment::Prefetch(url) => {
                            self.worker.url(mem::take(url))?;
                        }
                    }
                }

                last_duration.sleep(time.elapsed());
                self.init = false;
            }
            QueueRange::Back(newest) => {
                if !self.init {
                    info!("Failed to find next segment, skipping to newest...");
                }

                let newest = newest.context("Failed to find newest segment")?;
                debug!("Sending newest segment to worker:\n{newest:?}");

                match newest {
                    Segment::Normal(duration, url) => {
                        self.worker.url(mem::take(url))?;
                        duration.sleep(time.elapsed());
                    }
                    Segment::Prefetch(url) => self.worker.url(mem::take(url))?,
                }
            }
            QueueRange::Empty => {
                if last_duration < Duration::MAX && !self.init {
                    info!("Playlist unchanged, retrying...");
                }

                last_duration.sleep_half(time.elapsed());
            }
        }

        Ok(())
    }
}
