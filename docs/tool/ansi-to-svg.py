#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["rich>=13.7"]
# ///
"""Render an ANSI terminal capture (`tmux capture-pane -e`) as an SVG.

Rich's `save_svg` draws the capture inside a terminal window chrome, which is
what the existing README images use — keeping every terminal screenshot in
docs/images visually identical in font, padding and window frame.

Usage: ansi-to-svg.py <capture.ansi> <out.svg> [--title T] [--width N]
"""

from __future__ import annotations

import argparse
import io

from rich.console import Console
from rich.text import Text


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("input")
    ap.add_argument("output")
    ap.add_argument("--title", default="claude-commander")
    ap.add_argument("--width", type=int, default=120)
    args = ap.parse_args()

    with open(args.input, encoding="utf-8") as fh:
        capture = fh.read().rstrip("\n")

    console = Console(
        record=True,
        width=args.width,
        file=io.StringIO(),  # record only; nothing goes to stdout
        force_terminal=True,
        color_system="truecolor",
        legacy_windows=False,
    )
    # `end=""` keeps the capture's own trailing newline handling — an extra blank
    # row at the bottom would show up as dead space in the window chrome.
    console.print(Text.from_ansi(capture), end="")
    console.save_svg(args.output, title=args.title)


if __name__ == "__main__":
    main()
