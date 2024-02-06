use std::{ops::ControlFlow, time::Instant};

use anyhow::Result;
use log::{debug, info};

use super::{
    playlist::MediaPlaylist,
    segment::{PrefetchSegmentKind, Segment},
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
    prefetch_kind: PrefetchSegmentKind,
    was_unchanged: bool,
    init: bool,
}

impl SegmentHandler for LowLatency {
    fn new(playlist: MediaPlaylist, worker: Worker) -> Self {
        info!("Low latency streaming");
        Self {
            playlist,
            worker,
            prev_segment: Segment::default(),
            prefetch_kind: PrefetchSegmentKind::Newest,
            was_unchanged: bool::default(),
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
        match self.playlist.prefetch_segment(self.prefetch_kind) {
            Ok(segment) if self.prev_segment == segment => {
                if self.was_unchanged {
                    info!("Playlist unchanged, retrying...");
                    segment.duration.sleep_half(time.elapsed());
                } else {
                    //already have the next segment, send it
                    info!("Playlist unchanged, fetching next segment...");
                    let segment = self
                        .playlist
                        .prefetch_segment(PrefetchSegmentKind::Newest)?;

                    self.prev_segment = segment.clone();
                    self.worker.sync_url(segment.url)?;

                    self.was_unchanged = true;
                }

                Ok(())
            }
            Ok(mut segment) => {
                let (next, reached_end) = self.playlist.next_segment(&self.prev_segment)?;
                match next {
                    Some(next) => {
                        //no longer using prefetch urls
                        info!("Downgrading to normal latency handler");

                        self.prev_segment = next.clone();
                        self.worker.url(next.url)?;
                        next.duration.sleep(time.elapsed());

                        self.reload()?;
                        return Err(Error::Downgrade.into());
                    }
                    None if reached_end => {
                        //happy path
                        debug!("Next segment is next prefetch segment");
                    }
                    _ => {
                        if self.init {
                            self.init = false;
                        } else {
                            info!("Failed to find next segment, jumping to newest");
                        }

                        self.prefetch_kind = PrefetchSegmentKind::Newest;
                        segment = self.playlist.prefetch_segment(self.prefetch_kind)?;
                    }
                };
                self.was_unchanged = false;
                self.prev_segment = segment.clone();

                match self.prefetch_kind {
                    PrefetchSegmentKind::Newest => {
                        self.worker.sync_url(segment.url)?;
                        self.prefetch_kind = PrefetchSegmentKind::Next;
                        return Ok(());
                    }
                    PrefetchSegmentKind::Next => self.worker.url(segment.url)?,
                };

                segment.duration.sleep(time.elapsed());
                Ok(())
            }
            Err(e) => Err(e),
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
        let segment = match self.playlist.next_segment(&self.prev_segment)? {
            (Some(segment), _) => {
                if self.prev_segment == segment {
                    info!("Playlist unchanged, retrying...");
                    segment.duration.sleep_half(time.elapsed());

                    return Ok(());
                }

                segment
            }
            (None, _) => {
                if !self.should_sync {
                    info!("Failed to find next segment, jumping to newest");
                }

                self.playlist.last_segment()?
            }
        };

        self.prev_segment = segment.clone();
        self.worker.send(segment.url, self.should_sync)?;
        if self.should_sync {
            self.should_sync = false;
            return Ok(());
        }

        segment.duration.sleep(time.elapsed());
        Ok(())
    }
}
