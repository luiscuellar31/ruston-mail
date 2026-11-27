#!/usr/bin/env python3
"""Derive desktop PNGs and the Windows ICO from the existing logo on macOS."""

from pathlib import Path
import struct
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
APP_ID = "com.luiscuellar.ruston-mail"
SIZES = (16, 24, 32, 48, 64, 128, 256, 512)
# Only the ICO carries these: Windows' small and taskbar icons at 125% and
# 150% scaling, which it would otherwise shrink from the next size up.
ICO_ONLY_SIZES = (20, 30, 36, 40)
# The largest frame an ICO holds; its directory writes this size as 0.
LARGEST_ICO_FRAME = 256
# An ICO starts with reserved (0), type (1, an icon) and how many frames.
ICO_HEADER = struct.Struct("<HHH")
# Then one entry per frame: width, height, palette colours, reserved, colour
# planes, bits per pixel, the frame's length, and where it starts.
ICO_ENTRY = struct.Struct("<BBBBHHII")


def resample(size, output):
    subprocess.run(
        [
            "sips",
            "--resampleHeightWidth", str(size), str(size),
            str(ROOT / "assets/macos/ruston-mail-1024.png"),
            "--out", str(output),
        ],
        check=True,
        stdout=subprocess.DEVNULL,
    )


def main():
    frames = []
    for size in SIZES:
        output = ROOT / f"assets/icons/hicolor/{size}x{size}/apps/{APP_ID}.png"
        output.parent.mkdir(parents=True, exist_ok=True)
        resample(size, output)
        if size <= LARGEST_ICO_FRAME:
            frames.append((size, output.read_bytes()))
    with tempfile.TemporaryDirectory() as scratch:
        for size in ICO_ONLY_SIZES:
            output = Path(scratch) / f"{size}.png"
            resample(size, output)
            frames.append((size, output.read_bytes()))
    frames.sort()
    output = ROOT / "assets/windows/ruston-mail.ico"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(ico(frames))


def ico(frames):
    """An ICO of PNG `frames`, each a (size, bytes) pair, in their order."""
    header = ICO_HEADER.pack(0, 1, len(frames))
    directory = bytearray()
    offset = ICO_HEADER.size + ICO_ENTRY.size * len(frames)
    for size, png in frames:
        dimension = size if size < LARGEST_ICO_FRAME else 0
        directory.extend(ICO_ENTRY.pack(dimension, dimension, 0, 0, 1, 32, len(png), offset))
        offset += len(png)
    return header + directory + b"".join(png for _, png in frames)


if __name__ == "__main__":
    main()
