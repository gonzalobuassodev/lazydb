#!/usr/bin/env python3
"""Build a deterministic tar.xz compatible with the shipped v0.1.0 updater."""

import argparse
from pathlib import Path
import tarfile


class CompatibleTarInfo(tarfile.TarInfo):
    def tobuf(self, *args, **kwargs):
        header = super().tobuf(*args, **kwargs)
        if self.isdir():
            # tarfile adds '/' for DIRTYPE, but v0.1.0 rejects empty path parts.
            # USTAR has one header; retain DIRTYPE and checksum the edited bytes.
            header = bytearray(header)
            name = header[:100].rstrip(b"\0").rstrip(b"/")
            header[:100] = name.ljust(100, b"\0")
            header[148:156] = b" " * 8
            header[148:156] = f"{sum(header):06o}\0 ".encode("ascii")
            header = bytes(header)
        return header


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("staging", type=Path, help="package root directory")
    parser.add_argument("output", type=Path, help="output tar.xz (outside staging)")
    args = parser.parse_args()
    root = args.staging.absolute()
    if root.is_symlink() or not root.is_dir():
        parser.error("staging must be a directory, not a symlink")
    if args.output.resolve().is_relative_to(root.resolve()):
        parser.error("output must be outside staging")
    paths = [root, *sorted(root.rglob("*"))]
    for path in paths:
        name = path.relative_to(root.parent).as_posix()
        if any(part in ("", ".", "..") for part in name.split("/")) or any(
            char in name for char in "\0\\:"
        ):
            parser.error(f"unsafe archive path: {name}")
        if path.is_symlink() or not (path.is_dir() or path.is_file()):
            parser.error(f"unsupported archive entry: {name}")
    with tarfile.open(args.output, "w:xz", format=tarfile.USTAR_FORMAT, preset=9) as archive:
        for path in paths:
            info = CompatibleTarInfo(path.relative_to(root.parent).as_posix())
            info.mode = 0o755 if path.is_dir() or path.stat().st_mode & 0o111 else 0o644
            # TarInfo defaults give fixed mtime, uid/gid, and empty owner names.
            if path.is_dir():
                info.type = tarfile.DIRTYPE
                archive.addfile(info)
            else:
                info.size = path.stat().st_size
                with path.open("rb") as contents:
                    archive.addfile(info, contents)


if __name__ == "__main__":
    main()
