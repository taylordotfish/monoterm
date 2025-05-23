/*
 * Copyright (C) 2021-2022, 2024 taylor.fish <contact@taylor.fish>
 *
 * This file is part of Monoterm.
 *
 * Monoterm is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * Monoterm is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with Monoterm. If not, see <https://www.gnu.org/licenses/>.
 */

use std::env;
use std::ffi::{OsStr, OsString};
use std::mem;
use std::process::exit;

const USAGE: &str = "\
Usage: monoterm [options] <command> [args...]

Executes <command> while converting all terminal colors to monochrome.

Options:
  -b, --bold     Convert foreground colors to bold text
  -h, --help     Show this help message
  -v, --version  Show program version
";

/// Maximum length of a single SGR sequence, excluding the initial CSI and
/// the ending 'm'. Sequences longer than this length will be forwarded to the
/// parent terminal unmodified.
const SGR_MAX_LEN: usize = 128;

enum SgrState {
    Init,
    AfterEsc,
    AfterCsi,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Intensity {
    High,
    Low,
    Normal,
}

struct Filter {
    bold_colors: bool,
    state: SgrState,
    background_set: bool,
    video_reversed: bool,
    foreground_set: bool,
    intensity: Intensity,
    /// Stores the contents of possible in-progress SGR escape sequences.
    buffer: Vec<u8>,
}

impl Filter {
    pub fn new(bold_colors: bool) -> Self {
        Self {
            bold_colors,
            state: SgrState::Init,
            background_set: false,
            video_reversed: false,
            foreground_set: false,
            intensity: Intensity::Normal,
            buffer: Vec::new(),
        }
    }

    fn parent_video_reversed(&self) -> bool {
        self.background_set != self.video_reversed
    }

    fn parent_intensity(&self) -> Intensity {
        if self.intensity == Intensity::Normal
            && self.bold_colors
            && self.foreground_set
        {
            Intensity::High
        } else {
            self.intensity
        }
    }

    fn handle_sgr<F>(&mut self, mut write: F)
    where
        F: FnMut(&[u8]),
    {
        fn skip_38_48(mut iter: impl Iterator<Item = Option<u8>>) {
            match iter.next() {
                Some(Some(5)) => {
                    iter.next();
                }
                Some(Some(2)) => {
                    iter.next(); // r
                    iter.next(); // g
                    iter.next(); // b
                }
                _ => {}
            }
        }

        let mut iter = self.buffer.split(|b| *b == b';').map(|arg| {
            (arg, match arg {
                [] => Some(0),
                _ => (|| std::str::from_utf8(arg).ok()?.parse().ok())(),
            })
        });

        let mut any_written = false;
        let mut write_arg = |arg: &[u8]| {
            write(if mem::replace(&mut any_written, true) {
                b";"
            } else {
                b"\x1b["
            });
            write(arg);
        };

        let mut reversed = self.parent_video_reversed();
        let mut intensity = self.parent_intensity();
        while let Some((arg, n)) = iter.next() {
            match n {
                Some(0) => {
                    self.background_set = false;
                    self.video_reversed = false;
                    self.foreground_set = false;
                    self.intensity = Intensity::Normal;
                    reversed = false;
                    intensity = Intensity::Normal;
                    write_arg(b"0");
                }
                Some(1) => {
                    self.intensity = Intensity::High;
                }
                Some(2) => {
                    self.intensity = Intensity::Low;
                }
                Some(22) => {
                    self.intensity = Intensity::Normal;
                }
                Some(30..=37 | 90..=97) => {
                    self.foreground_set = true;
                }
                Some(38) => {
                    skip_38_48(iter.by_ref().map(|(_, n)| n));
                    self.foreground_set = true;
                }
                Some(39) => {
                    self.foreground_set = false;
                }
                Some(58 | 59) => {}
                Some(7) => {
                    self.video_reversed = true;
                }
                Some(27) => {
                    self.video_reversed = false;
                }
                Some(40..=47) => {
                    self.background_set = true;
                }
                Some(48) => {
                    skip_38_48(iter.by_ref().map(|(_, n)| n));
                    self.background_set = true;
                }
                Some(49) => {
                    self.background_set = false;
                }
                Some(100..=107) => {
                    self.background_set = true;
                }
                _ => {
                    write_arg(arg);
                }
            }
        }

        let new_reversed = self.parent_video_reversed();
        if new_reversed != reversed {
            write_arg(if new_reversed {
                b"7"
            } else {
                b"27"
            });
        }

        let new_intensity = self.parent_intensity();
        if new_intensity != intensity {
            write_arg(match new_intensity {
                Intensity::High => b"1",
                Intensity::Low => b"2",
                Intensity::Normal => b"22",
            });
        }

        if any_written {
            write(b"m");
        }
    }

    fn handle_byte<F>(&mut self, b: u8, mut write: F)
    where
        F: FnMut(&[u8]),
    {
        match &self.state {
            SgrState::Init => match b {
                0x1b => {
                    self.state = SgrState::AfterEsc;
                }
                b => write(&[b]),
            },
            SgrState::AfterEsc => match b {
                b'[' => {
                    self.state = SgrState::AfterCsi;
                    self.buffer.clear();
                }
                b => {
                    self.state = SgrState::Init;
                    write(&[0x1b, b]);
                }
            },
            SgrState::AfterCsi => match b {
                b'm' => {
                    self.state = SgrState::Init;
                    self.handle_sgr(write);
                }
                b'0'..=b'9' | b';' if self.buffer.len() < SGR_MAX_LEN => {
                    self.buffer.push(b);
                }
                b => {
                    self.state = SgrState::Init;
                    write(b"\x1b[");
                    write(&self.buffer);
                    write(&[b]);
                }
            },
        }
    }
}

impl filterm::Filter for Filter {
    fn on_child_data<F>(&mut self, data: &[u8], mut parent_write: F)
    where
        F: FnMut(&[u8]),
    {
        data.iter().copied().for_each(|b| {
            self.handle_byte(b, &mut parent_write);
        });
    }
}

fn show_usage() -> ! {
    print!("{USAGE}");
    exit(0);
}

fn show_version() -> ! {
    println!("{}", env!("CARGO_PKG_VERSION"));
    exit(0);
}

macro_rules! args_error {
    ($($args:tt)*) => {{
        eprintln!("error: {}", format_args!($($args)*));
        eprintln!("See `monoterm --help` for usage information.");
        exit(1);
    }};
}

struct ParsedArgs {
    pub command: Vec<OsString>,
    pub bold: bool,
}

fn parse_args<Args>(args: Args) -> ParsedArgs
where
    Args: IntoIterator<Item = OsString>,
{
    let mut bold = false;
    let mut options_done = false;

    // Returns whether `arg` should be part of the executed command.
    let mut process_arg = |arg: &OsStr| {
        let bytes = arg.as_encoded_bytes();
        if options_done || arg == "-" {
        } else if arg == "--" {
            options_done = true;
            return false;
        } else if arg == "--help" {
            show_usage();
        } else if arg == "--version" {
            show_version();
        } else if arg == "--bold" {
            bold = true;
            return false;
        } else if bytes.starts_with(b"--") {
            args_error!("unrecognized option: {}", arg.to_string_lossy());
        } else if let Some(opts) = bytes.strip_prefix(b"-") {
            opts.iter().copied().for_each(|opt| match opt {
                b'h' => show_usage(),
                b'v' => show_version(),
                b'b' => {
                    bold = true;
                }
                _ if opt.is_ascii() => {
                    args_error!("unrecognized option: -{}", char::from(opt));
                }
                _ => {
                    args_error!(
                        "unrecognized option: {}",
                        arg.to_string_lossy(),
                    );
                }
            });
            return false;
        }
        options_done = true;
        true
    };

    let command: Vec<_> =
        args.into_iter().filter(|a| process_arg(a)).collect();
    if command.is_empty() {
        eprint!("{USAGE}");
        exit(1);
    }
    ParsedArgs {
        command,
        bold,
    }
}

fn main() {
    let args = parse_args(env::args_os().skip(1));
    let mut filter = Filter::new(args.bold);
    if let Err(e) = filterm::run(args.command, &mut filter) {
        eprintln!("error: {e}");
        exit(1);
    }
}
