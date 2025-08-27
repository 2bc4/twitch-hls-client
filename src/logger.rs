use std::{
    env,
    io::{self, IsTerminal},
    time::SystemTime,
};

use anyhow::Result;
use log::{Level, LevelFilter, Log, Metadata, Record};

pub struct Logger {
    enable_debug: bool,
    enable_colors: bool,
}

impl Log for Logger {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        unreachable!();
    }

    fn log(&self, record: &Record<'_>) {
        let level = record.level();
        match level {
            Level::Error | Level::Info | Level::Debug if self.enable_debug => {
                let thread = std::thread::current();
                println!(
                    "{time} {tag} ({thread}) {module}: {log}",
                    time = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis(),
                    tag = level_tag(level, self.enable_colors),
                    thread = thread.name().unwrap_or("<unknown>"),
                    module = record.module_path().unwrap_or("<unknown>"),
                    log = record.args(),
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
