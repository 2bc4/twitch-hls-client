use std::{
    env,
    io::{self, IsTerminal},
    thread,
};

use anyhow::Result;
use log::{self, Level, LevelFilter, Log, Metadata, Record};
use time::{format_description::FormatItem, macros::format_description, OffsetDateTime, UtcOffset};

const TIME_FORMAT_DESCRIPTION: &[FormatItem<'static>] =
    format_description!("[hour]:[minute]:[second].[subsecond digits:5]");

pub struct Logger {
    enable_debug: bool,
    enable_colors: bool,
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
                    level_tag(level, self.enable_colors),
                    thread.name().unwrap_or_default(),
                    record.module_path().unwrap_or_default(),
                    record.args()
                );
            }
            Level::Error => eprintln!("{} {}", level_tag(level, self.enable_colors), record.args()),
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
            enable_colors: env::var_os("NO_COLOR").is_none() && io::stdout().is_terminal(),
            offset: UtcOffset::current_local_offset()?,
        }))?;

        let level_filter = if enable_debug {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        };
        log::set_max_level(level_filter);

        Ok(())
    }
}

fn level_tag_no_color(level: Level) -> &'static str {
    match level {
        Level::Error => "[ERROR]",
        Level::Info => "[INFO]",
        Level::Debug => "[DEBUG]",
        _ => unreachable!(),
    }
}

#[cfg(feature = "colors")]
fn level_tag(level: Level, enable_colors: bool) -> &'static str {
    if enable_colors {
        match level {
            Level::Error => "\x1b[31m[ERROR]\x1b[0m", //red
            Level::Info => "\x1b[34m[INFO]\x1b[0m",   //blue
            Level::Debug => "\x1b[36m[DEBUG]\x1b[0m", //cyan
            _ => unreachable!(),
        }
    } else {
        level_tag_no_color(level)
    }
}

#[cfg(not(feature = "colors"))]
fn level_tag(level: Level, _enable_colors: bool) -> &'static str {
    level_tag_no_color(level)
}
