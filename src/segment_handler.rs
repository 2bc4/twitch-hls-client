use std::{ops::ControlFlow, time::Instant};

use anyhow::{Context, Result};
use log::{debug, info};
use url::Url;

use crate::{
    hls::{MediaPlaylist, PrefetchSegment, Segment},
    worker::Worker,
};

pub trait SegmentHandler {
    fn new(playlist: MediaPlaylist, worker: Worker) -> Self;
    fn reload(&mut self) -> Result<()>;
    fn process(&mut self, time: Instant) -> Result<()>;
}

pub struct LowLatencyHandler {
    playlist: MediaPlaylist,
    worker: Worker,
    prev_url: String,
    prefetch_kind: PrefetchSegment,
    unchanged_count: u32,
}

impl SegmentHandler for LowLatencyHandler {
    fn new(playlist: MediaPlaylist, worker: Worker) -> Self {
        Self {
            playlist,
            worker,
            prev_url: String::default(),
            prefetch_kind: PrefetchSegment::Newest,
            unchanged_count: u32::default(),
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

impl LowLatencyHandler {
    fn handle_segment(&mut self, time: Instant) -> Result<()> {
        match self.playlist.prefetch_url(self.prefetch_kind) {
            Ok(url) if self.prev_url == url.as_str() => {
                if self.unchanged_count == 0 {
                    //already have the next segment, send it
                    info!("Playlist unchanged, fetching next segment...");
                    let url = self.playlist.prefetch_url(PrefetchSegment::Newest)?;
                    self.prev_url = url.as_str().to_owned();

                    self.worker.sync_url(url)?;
                } else {
                    info!("Playlist unchanged, retrying...");
                    self.playlist.last_duration()?.sleep_half(time.elapsed());
                }

                self.unchanged_count += 1;
                return Ok(());
            }
            Ok(mut url) => {
                //next segment may no longer be next prefetch segment
                if self.unchanged_count > 1 {
                    let (segment, is_last) = get_next_segment(&self.playlist, &self.prev_url)?;
                    match segment {
                        Some(segment) => {
                            debug!("Found next segment");

                            self.prev_url = segment.url.as_str().to_owned();
                            segment.duration.sleep(time.elapsed());

                            return Ok(());
                        }
                        None if is_last => {
                            debug!("Next segment is next prefetch segment");
                            self.unchanged_count = 0;
                        }
                        _ => {
                            url = self.jump_to_newest()?;
                        }
                    };
                }
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

    fn jump_to_newest(&mut self) -> Result<Url> {
        info!("Failed to find next segment, jumping to newest");
        self.prefetch_kind = PrefetchSegment::Newest;
        self.unchanged_count = 0;

        self.playlist.prefetch_url(self.prefetch_kind)
    }
}

pub struct NormalLatencyHandler {
    playlist: MediaPlaylist,
    worker: Worker,
    should_sync: bool,
    prev_url: String,
}

impl SegmentHandler for NormalLatencyHandler {
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

impl NormalLatencyHandler {
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
                let segments = self.playlist.segments()?;
                segments
                    .last()
                    .context("Failed to get latest segment")?
                    .clone()
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
            .get(idx + 1)
            .context("Failed to get next segment")?;

        return Ok((Some(segment.clone()), false));
    }

    Ok((None, false))
}
