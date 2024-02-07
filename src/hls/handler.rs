use std::{ops::ControlFlow, time::Instant};

use anyhow::{Context, Result};
use log::info;

use super::{
    playlist::MediaPlaylist,
    segment::{NextSegment, Segment},
    Error,
};

use crate::worker::Worker;

pub trait SegmentHandler {
    fn new(playlist: MediaPlaylist, worker: Worker) -> Self;
    fn reload(&mut self) -> Result<()>;
    fn process(&mut self, time: Instant) -> Result<()>;
}

pub struct LowLatency {
    playlist: MediaPlaylist,
    worker: Worker,
    prev_segment: Segment,
    init: bool,
}

impl SegmentHandler for LowLatency {
    fn new(playlist: MediaPlaylist, worker: Worker) -> Self {
        info!("Low latency streaming");
        Self {
            playlist,
            worker,
            prev_segment: Segment::default(),
            init: true,
        }
    }

    fn reload(&mut self) -> Result<()> {
        self.playlist.reload()
    }

    fn process(&mut self, time: Instant) -> Result<()> {
        match self.playlist.filter_if_ad(&time)? {
            ControlFlow::Continue(()) => self.handle_segment(time),
            ControlFlow::Break(()) => Ok(()),
        }
    }
}

impl LowLatency {
    pub fn downgrade(self) -> NormalLatency {
        let mut handler = NormalLatency::new(self.playlist, self.worker);
        handler.prev_segment = self.prev_segment;
        handler.should_sync = false;

        handler
    }

    fn handle_segment(&mut self, time: Instant) -> Result<()> {
        let segments = self.playlist.segments()?;
        match self.prev_segment.find_next(&segments)? {
            NextSegment::Found(segment) => {
                self.prev_segment = segment.clone();
                match segment {
                    Segment::Normal(duration, url) => {
                        //no longer using prefetch urls

                        self.worker.url(url)?;
                        duration.sleep(time.elapsed());

                        self.reload()?;
                        return Err(Error::Downgrade.into());
                    }
                    Segment::NextPrefetch(duration, url) => {
                        self.worker.url(url)?;
                        duration.sleep(time.elapsed());
                    }
                    Segment::NewestPrefetch(url) => self.worker.sync_url(url)?,
                    Segment::Unknown => unreachable!(),
                }

                Ok(())
            }
            NextSegment::Current => {
                playlist_unchanged(&self.prev_segment, time)?;
                Ok(())
            }
            NextSegment::Unknown => {
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

                Ok(())
            }
        }
    }
}

pub struct NormalLatency {
    playlist: MediaPlaylist,
    worker: Worker,
    prev_segment: Segment,
    should_sync: bool,
}

impl SegmentHandler for NormalLatency {
    fn new(playlist: MediaPlaylist, worker: Worker) -> Self {
        Self {
            playlist,
            worker,
            prev_segment: Segment::default(),
            should_sync: true,
        }
    }

    fn reload(&mut self) -> Result<()> {
        self.playlist.reload()
    }

    fn process(&mut self, time: Instant) -> Result<()> {
        match self.playlist.filter_if_ad(&time)? {
            ControlFlow::Continue(()) => self.handle_segment(time),
            ControlFlow::Break(()) => Ok(()),
        }
    }
}

impl NormalLatency {
    fn handle_segment(&mut self, time: Instant) -> Result<()> {
        let segments = self.playlist.segments()?;
        match self.prev_segment.find_next(&segments)? {
            NextSegment::Found(segment) => {
                self.prev_segment = segment.clone();
                match segment {
                    Segment::Normal(duration, url) => {
                        self.worker.send(url, self.should_sync)?;
                        duration.sleep(time.elapsed());
                    }
                    Segment::NextPrefetch(_, _) | Segment::NewestPrefetch(_) => {
                        playlist_unchanged(&self.prev_segment, time)?; //downgraded from LL handler
                    }
                    Segment::Unknown => unreachable!(),
                };

                Ok(())
            }
            NextSegment::Current => {
                playlist_unchanged(&self.prev_segment, time)?; //not downgraded
                Ok(())
            }
            NextSegment::Unknown => {
                if !self.should_sync {
                    info!("Failed to find next segment, jumping to newest...");
                }

                let segment = segments
                    .into_iter()
                    .rev()
                    .find(|s| matches!(s, Segment::Normal(_, _)))
                    .context("Failed to find last segment")?;

                self.prev_segment = segment.clone();
                let (duration, url) = segment.destructure();

                self.worker.send(url, self.should_sync)?;
                if self.should_sync {
                    self.should_sync = false;
                    return Ok(());
                }

                duration
                    .context("Invalid duration in segment")?
                    .sleep(time.elapsed());

                Ok(())
            }
        }
    }
}

fn playlist_unchanged(segment: &Segment, time: Instant) -> Result<()> {
    info!("Playlist unchanged, retrying...");
    let (duration, _) = segment.destructure_ref();
    duration
        .context("Failed to get duration from segment while retrying")?
        .sleep_half(time.elapsed());

    Ok(())
}
