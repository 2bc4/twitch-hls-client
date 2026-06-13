use std::{
    sync::mpsc::{self, Sender},
    thread::{Builder as ThreadBuilder, JoinHandle},
    time::Instant,
};

use anyhow::{Context, Result, bail};
use log::{debug, info};

use crate::{
    hls::{Duration, Playlist, QueueRange, Segment},
    http::{Connection, Method, Request, StatusError, Url},
    output::{Output, Writer},
};

pub fn run(mut writer: Writer, conn: Connection) -> Result<()> {
    let mut playlist = Playlist::new(conn)?;
    if let Some(url) = &playlist.header {
        let mut request = Request::new(Vec::new());
        request.call(Method::Get, url)?;

        writer.set_header(&request.into_writer())?;
    }

    if writer.should_wait() {
        writer.wait_for_output()?;
    }

    let mut worker = Some(Worker::spawn(Request::new(writer))?);
    let mut has_dispatched = false;
    loop {
        if process_segments(
            worker.as_ref().expect("Missing worker while processing"),
            &mut playlist,
            &mut has_dispatched,
        )? {
            let mut request = worker.take().expect("Missing worker before join").join()?;
            request.get_mut().wait_for_output()?;

            worker = Some(Worker::spawn(request)?);
            has_dispatched = false;
            playlist.reset();
        }
    }
}

fn process_segments(
    worker: &Worker,
    playlist: &mut Playlist,
    has_dispatched: &mut bool,
) -> Result<bool> {
    let start = Instant::now();
    playlist.reload()?;

    let last_duration = playlist
        .last_duration()
        .context("Failed to find last segment duration")?;

    if last_duration.is_ad {
        info!("Filtering ad segment...");
        last_duration.sleep(start.elapsed());

        return Ok(false);
    }

    match playlist.segment_queue() {
        QueueRange::Partial(ref mut segments) => {
            for segment in segments {
                debug!("Processing segment:\n{segment:?}");
                if !worker.send(segment.take_url()) {
                    return Ok(true);
                }
            }

            last_duration.sleep(start.elapsed());
            *has_dispatched = true;
        }
        QueueRange::Back(newest) => {
            if *has_dispatched {
                info!("Failed to find next segment, skipping to newest...");
            }

            let newest = newest.context("Failed to find newest segment")?;

            debug!("Processing newest segment:\n{newest:?}");
            if !worker.send(newest.take_url()) {
                return Ok(true);
            }

            if let Segment::Normal(duration, _) = newest {
                duration.sleep(start.elapsed());
            }
        }
        QueueRange::Empty => {
            if *has_dispatched && last_duration < Duration::MAX {
                info!("Playlist unchanged, retrying...");
            }

            last_duration.sleep_half(start.elapsed());
        }
    }

    Ok(false)
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
