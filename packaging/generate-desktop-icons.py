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
        if size <= 256:
            frames.append((size, output.read_bytes()))
    with tempfile.TemporaryDirectory() as scratch:
        for size in ICO_ONLY_SIZES:
            output = Path(scratch) / f"{size}.png"
            resample(size, output)
            frames.append((size, output.read_bytes()))
    frames.sort()

    # ICO supports PNG frames; a zero dimension denotes 256 pixels.
    header = struct.pack("<HHH", 0, 1, len(frames))
    directory = bytearray()
    offset = len(header) + 16 * len(frames)
    for size, png in frames:
        dimension = size if size < 256 else 0
        directory.extend(
            struct.pack("<BBBBHHII", dimension, dimension, 0, 0, 1, 32, len(png), offset)
        )
        offset += len(png)
    output = ROOT / "assets/windows/ruston-mail.ico"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_bytes(header + directory + b"".join(png for _, png in frames))


if __name__ == "__main__":
    main()
