use std::{
    cmp::Ordering,
    mem,
    str::FromStr,
    sync::mpsc::{self, Sender},
    thread::{self, Builder as ThreadBuilder, JoinHandle},
    time::{self, Instant},
};

use anyhow::{Context, Result, bail};
use log::{debug, info};

use super::playlist::{Playlist, QueueRange};
use crate::{
    http::{Method, Request, StatusError, Url},
    output::{Output, Writer},
};

pub struct Handler {
    worker: Option<Worker>,
    has_dispatched: bool,
    should_reset: bool,
}

impl Handler {
    pub fn new(mut writer: Writer, playlist: &Playlist) -> Result<Self> {
        if let Some(url) = &playlist.header {
            let mut request = Request::new(Vec::new());
            request.call(Method::Get, url)?;

            writer.set_header(&request.into_writer())?;
        }

        if writer.should_wait() {
            writer.wait_for_output()?;
        }

        Ok(Self {
            worker: Some(Worker::spawn(Request::new(writer))?),
            has_dispatched: bool::default(),
            should_reset: bool::default(),
        })
    }

    pub fn run(&mut self, playlist: &mut Playlist) -> Result<()> {
        loop {
            let start = Instant::now();
            playlist.reload()?;

            let last_duration = playlist
                .last_duration()
                .context("Failed to find last segment duration")?;

            if last_duration.is_ad {
                info!("Filtering ad segment...");
                last_duration.sleep(start.elapsed());

                continue;
            }

            match playlist.segment_queue() {
                QueueRange::Partial(ref mut segments) => {
                    for segment in segments {
                        debug!("Processing segment:\n{segment:?}");
                        match segment {
                            Segment::Normal(_, url) | Segment::Prefetch(url) => {
                                self.dispatch(url)?;
                            }
                        }
                    }

                    last_duration.sleep(start.elapsed());
                    self.has_dispatched = true;
                }
                QueueRange::Back(newest) => {
                    if self.has_dispatched {
                        info!("Failed to find next segment, skipping to newest...");
                    }

                    let newest = newest.context("Failed to find newest segment")?;
                    debug!("Processing newest segment:\n{newest:?}");

                    match newest {
                        Segment::Normal(duration, url) => {
                            self.dispatch(url)?;
                            duration.sleep(start.elapsed());
                        }
                        Segment::Prefetch(url) => self.dispatch(url)?,
                    }
                }
                QueueRange::Empty => {
                    if self.has_dispatched && last_duration < Duration::MAX {
                        info!("Playlist unchanged, retrying...");
                    }

                    last_duration.sleep_half(start.elapsed());
                }
            }

            if self.should_reset {
                playlist.reset();
                self.has_dispatched = false;
                self.should_reset = false;
            }
        }
    }

    fn dispatch(&mut self, url: &mut Url) -> Result<()> {
        if !self
            .worker
            .as_mut()
            .expect("Missing worker while sending URL")
            .send(mem::take(url))
        {
            let mut request = self
                .worker
                .take()
                .expect("Missing worker while joining")
                .join()?;

            request.get_mut().wait_for_output()?;

            self.worker = Some(Worker::spawn(request)?);
            self.should_reset = true;
        }

        Ok(())
    }
}

struct Worker {
    handle: JoinHandle<Result<Request<Writer>>>,
    sender: Sender<Url>,
}

impl Worker {
    fn spawn(mut request: Request<Writer>) -> Result<Self> {
        let (sender, receiver) = mpsc::channel::<Url>();
        let handle = ThreadBuilder::new()
            .name("hls worker".to_owned())
            .spawn(move || -> Result<Request<Writer>> {
                loop {
                    let Ok(url) = receiver.recv() else {
                        bail!("Worker died unexpectantly");
                    };

                    match request.call(Method::Get, &url) {
                        Ok(()) => (),
                        Err(e) if StatusError::is_not_found(&e) => {
                            info!("Segment not found, skipping ahead...");
                            receiver.try_iter().for_each(drop);
                        }
                        Err(e) => return Err(e),
                    }

                    if request.get_ref().should_wait() {
                        return Ok(request);
                    }
                }
            })
            .context("Failed to spawn worker")?;

        Ok(Self { handle, sender })
    }

    fn send(&self, url: Url) -> bool {
        self.sender.send(url).is_ok()
    }

    fn join(self) -> Result<Request<Writer>> {
        drop(self.sender);
        self.handle.join().expect("Worker panicked")
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
