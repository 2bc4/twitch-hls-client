use std::{
    cmp::Ordering,
    fmt::{self, Display, Formatter},
    mem,
    str::FromStr,
    thread,
    time::{self, Instant},
};

use anyhow::{Context, Result};
use log::{debug, info};

use super::{MediaPlaylist, media_playlist::QueueRange, worker::Worker};
use crate::{
    http::{Agent, Url},
    output::{Output, Writer},
};

#[derive(Debug)]
pub struct ResetError;

impl std::error::Error for ResetError {}

impl Display for ResetError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.write_str("Unhandled segment handler reset")
    }
}

pub struct Handler {
    worker: Option<Worker>,
    agent: Agent,
    init: bool,
}

impl Handler {
    pub fn new(writer: Writer, agent: Agent) -> Result<Self> {
        Ok(Self {
            worker: Some(Worker::spawn(writer, agent.clone())?),
            agent,
            init: true,
        })
    }

    pub const fn reset(&mut self) {
        self.init = true;
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

        match playlist.segment_queue() {
            QueueRange::Partial(ref mut segments) => {
                for segment in segments {
                    debug!("Processing segment:\n{segment:?}");
                    match segment {
                        Segment::Normal(_, url) | Segment::Prefetch(url) => self.dispatch(url)?,
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
                debug!("Processing newest segment:\n{newest:?}");

                match newest {
                    Segment::Normal(duration, url) => {
                        self.dispatch(url)?;
                        duration.sleep(time.elapsed());
                    }
                    Segment::Prefetch(url) => self.dispatch(url)?,
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

    fn dispatch(&mut self, url: &mut Url) -> Result<()> {
        if self
            .worker
            .as_mut()
            .expect("Missing worker while sending URL")
            .url(mem::take(url))
            .is_err()
        {
            match self.join_worker() {
                Ok(mut writer) => {
                    writer.wait_for_output()?;
                    self.worker = Some(Worker::spawn(writer, self.agent.clone())?);

                    return Err(ResetError.into());
                }
                Err(e) => return Err(e),
            }
        }

        Ok(())
    }

    fn join_worker(&mut self) -> Result<Writer> {
        self.worker
            .take()
            .expect("Missing worker while joining")
            .join()
    }
}

#[derive(Debug)]
pub enum Segment {
    Normal(Duration, Url),
    Prefetch(Url),
}

#[derive(Default, Copy, Clone, Debug)]
pub struct Duration {
    is_ad: bool,
    inner: time::Duration,
}

impl FromStr for Duration {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self {
            is_ad: s.contains('|'),
            inner: time::Duration::try_from_secs_f32(
                s.split_once(',')
                    .map(|d| d.0)
                    .and_then(|d| d.parse().ok())
                    .context("Invalid segment duration")?,
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
    //Can't wait too long or the server will close the socket
    const MAX: Self = Self {
        is_ad: false,
        inner: time::Duration::from_secs(3),
    };

    pub fn sleep(&self, elapsed: time::Duration) {
        if *self >= Self::MAX {
            self.sleep_half(elapsed);
            return;
        }

        Self::sleep_thread(self.inner, elapsed);
    }

    pub fn sleep_half(&self, elapsed: time::Duration) {
        if let Some(half) = self.inner.checked_div(2) {
            Self::sleep_thread(half, elapsed);
        }
    }

    fn sleep_thread(duration: time::Duration, elapsed: time::Duration) {
        if let Some(sleep_time) = duration.checked_sub(elapsed) {
            debug!("Sleeping thread for {sleep_time:?}");
            thread::sleep(sleep_time);
        }
    }
}
