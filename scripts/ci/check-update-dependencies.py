#!/usr/bin/env python3
"""Assert that production archive decoders are selected only per OS target."""

from __future__ import annotations

import argparse
import subprocess
import sys


FORBIDDEN = ("zopfli", "deflate64", "lzma-rust2", "ppmd-rust", "bzip2 v", "zstd v")


def check(target: str, tree: str) -> None:
    is_windows = "windows" in target
    has_zip = "zip v6." in tree
    has_tar = "tar v0." in tree
    has_xz = "xz2 v0." in tree
    if is_windows:
        if not has_zip or has_tar or has_xz:
            raise ValueError("Windows normal dependencies must include ZIP only, not tar/xz2")
    elif has_zip or not has_tar or not has_xz:
        raise ValueError("Unix normal dependencies must include tar/xz2 only, not ZIP")
    unwanted = [name for name in FORBIDDEN if name in tree]
    if unwanted:
        raise ValueError("unneeded ZIP decoder features are present: " + ", ".join(unwanted))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    args = parser.parse_args()
    try:
        result = subprocess.run(
            ["cargo", "tree", "--locked", "--target", args.target, "-e", "normal"],
            check=True,
            capture_output=True,
            text=True,
            timeout=60,
        )
        check(args.target, result.stdout)
    except (OSError, subprocess.SubprocessError, ValueError) as error:
        print(f"update dependency check failed: {error}", file=sys.stderr)
        return 1
    print(f"update decoder dependencies are target-minimized: {args.target}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
