"""Generate the Phase 6 image corpus.

Run by hand when a sample has to change; the *committed bytes* are the
fixture. Nothing in `cargo test` runs this script or Pillow — the point of
committing it is that every sample in this directory can be audited and
regenerated rather than taken on trust.

Every image is built from fixed pixels, never from randomness, so two runs of
this script with the same Pillow version produce the same file.

Sizes are deliberately not all multiples of four, because a decoder that gets
the row stride wrong still succeeds on 8x8 and fails on 7x5:

    odd.png          7 x 5   PNG    lossless, top-left asserted
    odd.bmp          9 x 4   BMP    24bpp, row padded to 4 bytes, top-left asserted
    small.gif        5 x 3   GIF    palette, top-left asserted
    block.jpg        8 x 8   JPEG   lossy, dimensions only
    unicode ….png    3 x 3   PNG    non-ASCII file name
    panel.png      320 x 200 PNG    the one a person can actually look at
    truncated.png      —     PNG    header cut in half, must not decode
"""

from pathlib import Path
import sys

from PIL import Image

ROOT = Path(__file__).resolve().parent

# Asserted exactly by every lossless sample, so a decoder that reads the
# wrong pixel order fails loudly instead of decoding something plausible.
TOP_LEFT = (200, 60, 10)
SECOND = (30, 90, 200)


def painted(size: tuple[int, int]) -> Image.Image:
    image = Image.new("RGB", size, TOP_LEFT)
    pixels = image.load()
    width, height = size

    for y in range(height):
        for x in range(width):
            if (x + y) % 3 == 0:
                pixels[x, y] = SECOND

    pixels[0, 0] = TOP_LEFT
    return image


def main() -> None:
    # One sample is named with an emoji, and a console on a code page that
    # cannot represent it must not take the whole run down over a print.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")

    ROOT.mkdir(parents=True, exist_ok=True)

    painted((7, 5)).save(ROOT / "odd.png", format="PNG")
    painted((9, 4)).save(ROOT / "odd.bmp", format="BMP")
    painted((5, 3)).save(ROOT / "small.gif", format="GIF")
    painted((8, 8)).save(
        ROOT / "block.jpg", format="JPEG", quality=100, subsampling=0
    )
    painted((3, 3)).save(ROOT / "unicode 中文 😊.png", format="PNG")

    # Big enough to look at. The samples above exist to catch stride and
    # channel-order bugs, and a 7x5 window proves nothing to a person; this
    # one is what a smoke test puts on screen. Red ramps left to right and
    # green top to bottom, so a buffer drawn upside down or with its channels
    # swapped is obvious without measuring anything.
    panel = Image.new("RGB", (320, 200))
    panel_pixels = panel.load()
    for y in range(200):
        for x in range(320):
            panel_pixels[x, y] = (x * 255 // 319, y * 255 // 199, 128)
    panel_pixels[0, 0] = TOP_LEFT
    panel.save(ROOT / "panel.png", format="PNG")

    # Cut inside the header, not at the tail. A PNG whose IDAT merely stops
    # halfway is tolerated by WIC: it returns the rows it managed to read and
    # zeroes the rest, so a test built on that would be asserting an
    # implementation detail of the OS rather than a fact about Toss. A file
    # that stops before the header is finished is refused, and that is the
    # claim worth testing.
    whole = (ROOT / "odd.png").read_bytes()
    (ROOT / "truncated.png").write_bytes(whole[: int(len(whole) * 0.3)])

    for path in sorted(ROOT.iterdir()):
        if path.suffix.lower() in {".png", ".jpg", ".bmp", ".gif"}:
            print(f"{path.name}: {path.stat().st_size} bytes")


if __name__ == "__main__":
    main()
