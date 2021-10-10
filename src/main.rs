/*
 * Copyright (C) 2021 taylor.fish <contact@taylor.fish>
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
use std::cell::Cell;
use std::env;
use std::ffi::OsString;
use std::mem;
use std::process::exit;

const USAGE: &str = "\
Usage:
  monoterm <command> [args...]
  monoterm [options]

Executes <command> while converting all terminal colors to monochrome.

Options:
  -h --help     Show this help message
  -v --version  Show program version
";

thread_local! {
    static BUFFER: Cell<Option<Vec<u8>>> = Cell::new(None);
}

fn get_buffer() -> Vec<u8> {
    BUFFER.with(|buf| buf.take().unwrap_or_else(Vec::new))
}

fn set_buffer(mut buffer: Vec<u8>) {
    buffer.clear();
    BUFFER.with(|buf| buf.set(Some(buffer)));
}

enum State {
    Init,
    AfterEsc,
    AfterCsi(Vec<u8>),
}

struct Filter {
    state: State,
    background_set: bool,
    video_reversed: bool,
}

impl Filter {
    pub fn new() -> Self {
        Self {
            state: State::Init,
            background_set: false,
            video_reversed: false,
        }
    }

    fn parent_video_reversed(&self) -> bool {
        self.background_set != self.video_reversed
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

        write(b"\x1b[");
        let mut reversed = self.parent_video_reversed();
        let mut any_written = false;
        let mut iter = data.split(|b| *b == b';').map(|arg| {
            (
                arg,
                match arg {
                    [] => Some(0),
                    _ => std::str::from_utf8(arg).unwrap().parse::<u8>().ok(),
                },
            )
        });

        let mut write_arg = |arg: &[u8]| {
            if mem::replace(&mut any_written, true) {
                write(b";");
            }
            write(arg);
        };

        while let Some((arg, n)) = iter.next() {
            match n {
                Some(0) => {
                    self.background_set = false;
                    self.video_reversed = false;
                    reversed = false;
                    write_arg(b"0");
                }

                Some(1 | 2 | 30..=37 | 39 | 58 | 59 | 90..=97) => {}
                Some(38) => skip_38_48(iter.by_ref().map(|(_, n)| n)),

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
        if new_reversed != reversed || !any_written {
            if any_written {
                write(b";");
            }
            write(if new_reversed {
                b"7"
            } else {
                b"27"
            });
        }
        write(b"m");
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
                    self.state = State::AfterCsi(get_buffer());
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
                    set_buffer(buf);
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
                    set_buffer(buf);
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

fn parse_args<Args>(args: Args) -> Vec<OsString>
where
    Args: IntoIterator<Item = OsString>,
{
    let mut options_done = false;
    let mut process_arg = |arg: &str| match arg {
        _ if options_done => true,
        "--" => {
            options_done = true;
            false
        }
        "--help" => show_usage(),
        "--version" => show_version(),
        s if s.starts_with("--") => {
            args_error!("unrecognized option: {}", s);
        }
        s if s.starts_with('-') => {
            s.chars().skip(1).for_each(|c| match c {
                'h' => show_usage(),
                'v' => show_version(),
                c => {
                    args_error!("unrecognized option: -{}", c);
                }
            });
            true
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
    command
}

fn main() {
    let args = parse_args(env::args_os().skip(1));
    let mut filter = Filter::new();
    match filterm::run(args, &mut filter) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("error: {}", e);
            exit(1);
        }
    }
}
