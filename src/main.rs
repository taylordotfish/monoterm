/*
 * Copyright (C) 2021-2022 taylor.fish <contact@taylor.fish>
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

use filterm::FilterHooks;
use std::env;
use std::ffi::OsString;
use std::mem;
use std::process::exit;

const USAGE: &str = "\
Usage: monoterm [options] <command> [args...]

Executes <command> while converting all terminal colors to monochrome.

Options:
  -b --bold     Convert foreground colors to bold text
  -h --help     Show this help message
  -v --version  Show program version
";

mod buffer {
    use std::cell::Cell;

    thread_local! {
        static CACHE: Cell<Option<Vec<u8>>> = Cell::new(None);
    }

    pub fn make_buffer() -> Vec<u8> {
        CACHE.with(|buf| buf.take().unwrap_or_default())
    }

    pub fn destroy_buffer(mut buffer: Vec<u8>) {
        buffer.clear();
        CACHE.with(|buf| buf.set(Some(buffer)));
    }
}

use buffer::{destroy_buffer, make_buffer};

enum State {
    Init,
    AfterEsc,
    AfterCsi(Vec<u8>),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Intensity {
    High,
    Low,
    Normal,
}

struct Filter {
    bold_colors: bool,
    state: State,
    background_set: bool,
    video_reversed: bool,
    foreground_set: bool,
    intensity: Intensity,
}

impl Filter {
    pub fn new(bold_colors: bool) -> Self {
        Self {
            bold_colors,
            state: State::Init,
            background_set: false,
            video_reversed: false,
            foreground_set: false,
            intensity: Intensity::Normal,
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

    fn handle_sgr<F>(&mut self, data: &[u8], mut write: F)
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

        let mut iter = data.split(|b| *b == b';').map(|arg| {
            (
                arg,
                match arg {
                    [] => Some(0),
                    _ => std::str::from_utf8(arg).unwrap().parse::<u8>().ok(),
                },
            )
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
        match &mut self.state {
            State::Init => match b {
                0x1b => {
                    self.state = State::AfterEsc;
                }
                b => write(&[b]),
            },
            State::AfterEsc => match b {
                b'[' => {
                    self.state = State::AfterCsi(make_buffer());
                }
                b => {
                    self.state = State::Init;
                    write(&[0x1b, b]);
                }
            },
            State::AfterCsi(buf) => match b {
                b'm' => {
                    let buf = mem::take(buf);
                    self.state = State::Init;
                    self.handle_sgr(&buf, write);
                    destroy_buffer(buf);
                }
                b'0'..=b'9' | b';' if buf.len() < 128 => {
                    buf.push(b);
                }
                b => {
                    let buf = mem::take(buf);
                    self.state = State::Init;
                    write(b"\x1b[");
                    write(&buf);
                    write(&[b]);
                    destroy_buffer(buf);
                }
            },
        }
    }
}

impl FilterHooks for Filter {
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
    print!("{}", USAGE);
    exit(0);
}

fn show_version() -> ! {
    println!("{}", env!("CARGO_PKG_VERSION"));
    exit(0);
}

macro_rules! args_error {
    ($($args:tt)*) => {
        eprintln!("error: {}", format_args!($($args)*));
        eprintln!("See monoterm --help for usage information.");
        exit(1);
    };
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
    let mut process_arg = |arg: &str| match arg {
        _ if options_done => true,
        "--" => {
            options_done = true;
            false
        }
        "--help" => show_usage(),
        "--version" => show_version(),
        "--bold" => {
            bold = true;
            false
        }
        s if s.starts_with("--") => {
            args_error!("unrecognized option: {}", s);
        }
        s if s.starts_with('-') => {
            let iter = s.chars().skip(1).map(|c| match c {
                'h' => show_usage(),
                'v' => show_version(),
                'b' => {
                    bold = true;
                }
                c => {
                    args_error!("unrecognized option: -{}", c);
                }
            });
            iter.fold(true, |_, _| false)
        }
        _ => {
            options_done = true;
            true
        }
    };

    let command: Vec<_> = args
        .into_iter()
        .filter(|a| process_arg(&a.to_string_lossy()))
        .collect();
    if command.is_empty() {
        eprint!("{}", USAGE);
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
        eprintln!("error: {}", e);
        exit(1);
    }
}
