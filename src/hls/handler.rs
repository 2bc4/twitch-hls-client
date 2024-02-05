use std::{ops::ControlFlow, time::Instant};

use anyhow::Result;
use log::{debug, info};

use super::Error;
use super::{playlist::MediaPlaylist, segment::PrefetchSegmentKind};

use crate::worker::Worker;

pub trait SegmentHandler {
    fn new(playlist: MediaPlaylist, worker: Worker) -> Self;
    fn reload(&mut self) -> Result<()>;
    fn process(&mut self, time: Instant) -> Result<()>;
}

pub struct LowLatency {
    playlist: MediaPlaylist,
    worker: Worker,
    prev_url: String,
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
            prev_url: String::default(),
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
        handler.prev_url = self.prev_url;
        handler.should_sync = false;

        handler
    }

    fn handle_segment(&mut self, time: Instant) -> Result<()> {
        match self.playlist.prefetch_segment(self.prefetch_kind) {
            Ok(segment) if self.prev_url == segment.url.as_str() => {
                if self.was_unchanged {
                    info!("Playlist unchanged, retrying...");
                    segment.duration.sleep_half(time.elapsed());
                } else {
                    //already have the next segment, send it
                    info!("Playlist unchanged, fetching next segment...");
                    let url = self
                        .playlist
                        .prefetch_segment(PrefetchSegmentKind::Newest)?
                        .url;
                    self.prev_url = url.as_str().to_owned();

                    self.worker.sync_url(url)?;
                    self.was_unchanged = true;
                }

                return Ok(());
            }
            Ok(mut segment) => {
                let (next, reached_end) = self.playlist.next_segment(&self.prev_url)?;
                match next {
                    Some(next) => {
                        //no longer using prefetch urls
                        info!("Downgrading to normal latency handler");

                        self.prev_url = next.url.as_str().to_owned();
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
                self.prev_url = segment.url.as_str().to_owned();

                match self.prefetch_kind {
                    PrefetchSegmentKind::Newest => {
                        self.worker.sync_url(segment.url)?;
                        self.prefetch_kind = PrefetchSegmentKind::Next;
                        return Ok(());
                    }
                    PrefetchSegmentKind::Next => self.worker.url(segment.url)?,
                };

                segment.duration.sleep(time.elapsed());
            }
            Err(e) => return Err(e),
        };

        Ok(())
    }
}

pub struct NormalLatency {
    playlist: MediaPlaylist,
    worker: Worker,
    should_sync: bool,
    prev_url: String,
}

impl SegmentHandler for NormalLatency {
    fn new(playlist: MediaPlaylist, worker: Worker) -> Self {
        Self {
            playlist,
            worker,
            should_sync: true,
            prev_url: String::default(),
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
        let segment = match self.playlist.next_segment(&self.prev_url)? {
            (Some(segment), _) => {
                if self.prev_url == segment.url.as_str() {
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

        self.prev_url = segment.url.as_str().to_owned();
        self.worker.send(segment.url, self.should_sync)?;
        if self.should_sync {
            self.should_sync = false;
            return Ok(());
        }

        segment.duration.sleep(time.elapsed());
        Ok(())
    }
}
