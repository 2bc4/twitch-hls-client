use std::{ops::ControlFlow, time::Instant};

use anyhow::{Context, Result};
use log::{debug, info};

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
        let segments = self.playlist.segments()?;
        match filter_if_ad(&segments, &time) {
            ControlFlow::Continue(()) => (),
            ControlFlow::Break(()) => return Ok(()),
        }

        match self.prev_segment.find_next(&segments)? {
            NextSegment::Found(segment) => {
                self.prev_segment = segment.clone();
                match segment {
                    Segment::Normal(duration, url) => {
                        //no longer using prefetch urls
                        debug!("Downgrading to normal latency handler");

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

impl LowLatency {
    pub fn downgrade(self) -> NormalLatency {
        let mut handler = NormalLatency::new(self.playlist, self.worker);
        handler.prev_segment = self.prev_segment;
        handler.should_sync = false;

        handler
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
        let segments = self.playlist.segments()?;
        match filter_if_ad(&segments, &time) {
            ControlFlow::Continue(()) => (),
            ControlFlow::Break(()) => return Ok(()),
        }

        match self.prev_segment.find_next(&segments)? {
            NextSegment::Found(segment) => {
                self.prev_segment = segment.clone();
                match segment {
                    Segment::Normal(duration, url) => {
                        self.worker.url(url)?;
                        duration.sleep(time.elapsed());
                    }
                    Segment::NextPrefetch(_, _) | Segment::NewestPrefetch(_) => {
                        playlist_unchanged(&self.prev_segment, time)?; //downgraded from LL handler
                    }
                    Segment::Unknown => unreachable!(),
                }

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
                    .context("Failed to find newest segment")?;

                self.prev_segment = segment.clone();
                let (duration, url) = segment.destructure();

                self.worker.send(url, self.should_sync)?;
                if self.should_sync {
                    self.should_sync = false;
                    return Ok(());
                }

                duration
                    .context("Failed to find segment duration")?
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
        .context("Failed to find segment duration from segment while retrying")?
        .sleep_half(time.elapsed());

    Ok(())
}

fn filter_if_ad(segments: &[Segment], time: &Instant) -> ControlFlow<()> {
    let duration = segments.iter().rev().find_map(|s| match s {
        Segment::Normal(duration, _) => Some(duration),
        _ => None,
    });

    if let Some(duration) = duration {
        if duration.is_ad {
            info!("Filtering ad segment...");
            duration.sleep(time.elapsed());

            return ControlFlow::Break(());
        }
    }

    ControlFlow::Continue(())
}
