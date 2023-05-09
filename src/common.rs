//    Copyright (C) 2023 2bc4
//
//    This program is free software: you can redistribute it and/or modify
//    it under the terms of the GNU General Public License as published by
//    the Free Software Foundation, either version 3 of the License, or
//    (at your option) any later version.
//
//    This program is distributed in the hope that it will be useful,
//    but WITHOUT ANY WARRANTY; without even the implied warranty of
//    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
//    GNU General Public License for more details.
//
//    You should have received a copy of the GNU General Public License
//    along with this program.  If not, see <https://www.gnu.org/licenses/>.

use anyhow::{Context, Result};
use log::info;
use std::process::{Child, Command, Stdio};

pub fn spawn_player(player_path: &str, player_args: &str) -> Result<Child> {
    info!("Opening player: {} {}", player_path, player_args);
    Command::new(player_path)
        .args(player_args.split_whitespace())
        .stdin(Stdio::piped())
        .spawn()
        .context("Failed to open player")
}
