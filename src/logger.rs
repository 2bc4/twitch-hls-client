use std::{
    env,
    io::{self, IsTerminal},
};

use anyhow::Result;
use log::{Level, LevelFilter, Log, Metadata, Record};

#[allow(dead_code)] //.enable_debug if debug logging feature disabled
pub struct Logger {
    enable_debug: bool,
    enable_colors: bool,
}

impl Log for Logger {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        unimplemented!(); //no need
    }

    fn log(&self, record: &Record<'_>) {
        let level = record.level();
        match level {
            #[cfg(feature = "debug-logging")]
            Level::Error | Level::Info | Level::Debug if self.enable_debug => {
                use std::time::{Duration, SystemTime};

                let thread = std::thread::current();
                println!(
                    "{} {} ({}) {}: {}",
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or(Duration::ZERO)
                        .as_millis(),
                    level_tag(level, self.enable_colors),
                    thread.name().unwrap_or("<unknown>"),
                    record.module_path().unwrap_or("<unknown>"),
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
        }))?;

        log::set_max_level(if enable_debug {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        });

        #[cfg(not(feature = "debug-logging"))]
        if enable_debug {
            log::info!("Debug logging was disabled at build time");
        }

        Ok(())
    }
}

pub fn is_debug() -> bool {
    log::max_level() == LevelFilter::Debug
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
