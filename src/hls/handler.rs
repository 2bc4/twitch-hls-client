use std::{ops::ControlFlow, time::Instant};

use anyhow::{Context, Result};
use log::{debug, error, info};

use super::Error;

use super::{
    playlist::MediaPlaylist,
    segment::{PrefetchSegment, Segment},
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
    prev_url: String,
    prefetch_kind: PrefetchSegment,
    was_unchanged: bool,
}

impl SegmentHandler for LowLatency {
    fn new(playlist: MediaPlaylist, worker: Worker) -> Self {
        info!("Low latency streaming");
        Self {
            playlist,
            worker,
            prev_url: String::default(),
            prefetch_kind: PrefetchSegment::Newest,
            was_unchanged: bool::default(),
        }
    }

    fn reload(&mut self) -> Result<()> {
        self.playlist.reload()
    }

    fn process(&mut self, time: Instant) -> Result<()> {
        match filter_if_ad(&self.playlist, &time)? {
            ControlFlow::Continue(()) => self.handle_segment(time),
            ControlFlow::Break(()) => Ok(()),
        }
    }
}

impl LowLatency {
    pub fn downgrade(self) -> NormalLatency {
        let mut handler = NormalLatency::new(self.playlist, self.worker);
        handler.prev_url = self.prev_url;

        handler
    }

    fn handle_segment(&mut self, time: Instant) -> Result<()> {
        match self.playlist.prefetch_url(self.prefetch_kind) {
            Ok(url) if self.prev_url == url.as_str() => {
                if self.was_unchanged {
                    info!("Playlist unchanged, retrying...");
                    self.playlist.last_duration()?.sleep_half(time.elapsed());
                } else {
                    //already have the next segment, send it
                    info!("Playlist unchanged, fetching next segment...");
                    let url = self.playlist.prefetch_url(PrefetchSegment::Newest)?;
                    self.prev_url = url.as_str().to_owned();

                    self.worker.sync_url(url)?;
                    self.was_unchanged = true;
                }

                return Ok(());
            }
            Ok(mut url) => {
                let (segment, is_last) = get_next_segment(&self.playlist, &self.prev_url)?;
                match segment {
                    Some(segment) => {
                        //no longer using prefetch urls
                        info!("Downgrading to normal latency handler");

                        self.prev_url = segment.url.as_str().to_owned();
                        self.worker.url(segment.url)?;
                        segment.duration.sleep(time.elapsed());

                        return Err(Error::Downgrade.into());
                    }
                    None if is_last => {
                        //happy path
                        debug!("Next segment is next prefetch segment");
                    }
                    _ => {
                        error!("Failed to find next segment, jumping to newest");
                        self.prefetch_kind = PrefetchSegment::Newest;
                        url = self.playlist.prefetch_url(self.prefetch_kind)?;
                    }
                };
                self.was_unchanged = false;
                self.prev_url = url.as_str().to_owned();

                match self.prefetch_kind {
                    PrefetchSegment::Newest => {
                        self.worker.sync_url(url)?;
                        self.prefetch_kind = PrefetchSegment::Next;
                        return Ok(());
                    }
                    PrefetchSegment::Next => self.worker.url(url)?,
                };
            }
            Err(e) => return Err(e),
        };

        self.playlist.last_duration()?.sleep(time.elapsed());
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
        match filter_if_ad(&self.playlist, &time)? {
            ControlFlow::Continue(()) => self.handle_segment(time),
            ControlFlow::Break(()) => Ok(()),
        }
    }
}

impl NormalLatency {
    fn handle_segment(&mut self, time: Instant) -> Result<()> {
        let segment = match get_next_segment(&self.playlist, &self.prev_url)? {
            (Some(segment), _) => {
                if self.prev_url == segment.url.as_str() {
                    info!("Playlist unchanged, retrying...");
                    self.playlist.last_duration()?.sleep_half(time.elapsed());

                    return Ok(());
                }

                segment
            }
            (None, _) => {
                error!("Failed to find next segment, jumping to newest");
                let segments = self.playlist.segments()?;

                segments
                    .into_iter()
                    .last()
                    .context("Failed to get latest segment")?
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

fn filter_if_ad(playlist: &MediaPlaylist, time: &Instant) -> Result<ControlFlow<()>> {
    if playlist.has_ad() {
        info!("Filtering ad segment...");
        playlist.last_duration()?.sleep(time.elapsed());

        return Ok(ControlFlow::Break(()));
    }

    Ok(ControlFlow::Continue(()))
}

fn get_next_segment(playlist: &MediaPlaylist, prev_url: &str) -> Result<(Option<Segment>, bool)> {
    let segments = playlist.segments()?;
    if let Some(idx) = segments.iter().position(|s| prev_url == s.url.as_str()) {
        if idx + 1 == segments.len() {
            return Ok((None, true));
        }

        let segment = segments
            .into_iter()
            .nth(idx + 1)
            .context("Failed to get next segment")?;

        return Ok((Some(segment), false));
    }

    Ok((None, false))
}
