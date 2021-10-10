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
use std::mem;
use std::process::exit;

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
        fn skip_38_48<'a>(mut iter: impl Iterator<Item = Option<u8>>) {
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
        let reversed = self.parent_video_reversed();
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

        while let Some((arg, n)) = iter.next() {
            match n {
                Some(0) => {
                    self.background_set = false;
                    self.video_reversed = false;
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
                    if mem::replace(&mut any_written, true) {
                        write(b";");
                    }
                    write(arg);
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

fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.is_empty() {
        eprintln!(
            "usage: {} <command> [args...]",
            env::current_exe()
                .ok()
                .as_ref()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("monoterm"),
        );
        exit(1);
    }

    let mut filter = Filter::new();
    match filterm::run(args, &mut filter) {
        Ok(_) => {}
        Err(e) => {
            eprintln!("error: {}", e);
            exit(1);
        }
    }
}
