use std::thread;

use anyhow::Result;
use log::{self, Level, LevelFilter, Log, Metadata, Record};
use time::{format_description::FormatItem, macros::format_description, OffsetDateTime, UtcOffset};

static TIME_FORMAT_DESCRIPTION: &[FormatItem<'static>] =
    format_description!("[hour]:[minute]:[second].[subsecond digits:5]");

pub struct Logger {
    enable_debug: bool,
    offset: UtcOffset,
}

impl Log for Logger {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        unimplemented!(); //no need
    }

    fn log(&self, record: &Record<'_>) {
        let level = record.level();
        match level {
            Level::Error | Level::Info | Level::Debug if self.enable_debug => {
                let thread = thread::current();
                println!(
                    "{} {} ({}) {}: {}",
                    OffsetDateTime::now_utc()
                        .to_offset(self.offset)
                        .format(&TIME_FORMAT_DESCRIPTION)
                        .unwrap(), //will never error
                    level_tag(level),
                    thread.name().unwrap_or_default(),
                    record.module_path().unwrap_or_default(),
                    record.args()
                );
            }
            Level::Error => eprintln!("{} {}", level_tag(level), record.args()),
            Level::Info => println!("{}", record.args()),
            _ => (),
        }
    }

    fn flush(&self) {}
}

impl Logger {
    pub fn init(enable_debug: bool) -> Result<()> {
        log::set_boxed_logger(Box::new(Self {
            enable_debug,
            offset: UtcOffset::current_local_offset()?,
        }))?;

        if enable_debug {
            log::set_max_level(LevelFilter::Debug);
        } else {
            log::set_max_level(LevelFilter::Info);
        }

        Ok(())
    }
}

fn level_tag_no_color(level: Level) -> &'static str {
    match level {
        Level::Error => "[ERROR]",
        Level::Info => "[INFO]",
        Level::Debug => "[DEBUG]",
        _ => unimplemented!(),
    }
}

#[cfg(feature = "colors")]
fn level_tag(level: Level) -> &'static str {
    use std::io::IsTerminal;
    if std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal() {
        match level {
            Level::Error => "\x1b[31m[ERROR]\x1b[0m", //red
            Level::Info => "\x1b[34m[INFO]\x1b[0m",   //blue
            Level::Debug => "\x1b[36m[DEBUG]\x1b[0m", //cyan
            _ => unimplemented!(),
        }
    } else {
        level_tag_no_color(level)
    }
}

#[cfg(not(feature = "colors"))]
fn level_tag(level: Level) -> &'static str {
    level_tag_no_color(level)
}
