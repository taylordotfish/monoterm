Monoterm
========

Monoterm converts all terminal colors to monochrome.

![A screenshot of a terminal with a line separating the image into two parts.
The left side includes many colors, while the right side is entirely black and
white.](misc/monoterm.png)

Why?
----

I initially developed Monoterm to use with my e-ink display. Since it converts
everything to grayscale, terminal colors simply make text harder to read. There
may be accessibility uses for Monoterm as well.

Installation
------------

Install with [Cargo](https://doc.rust-lang.org/cargo/):

```bash
cargo install monoterm
```

Usage
-----

The basic usage is `monoterm <command> [args...]`. Generally you would use this
to invoke your shell; e.g., `monoterm bash`.

With the `--bold` option, text that was originally colored will be rendered as
**bold**. See `monoterm --help` for more information and additional options.

License
-------

Monoterm is licensed under version 3 of the GNU General Public License,
or (at your option) any later version. See [LICENSE](LICENSE).

Contributing
------------

By contributing to Monoterm, you agree that your contribution may be used
according to the terms of Monoterm’s license.
