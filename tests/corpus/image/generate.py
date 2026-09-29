"""Generate the Phase 6 image corpus.

Run by hand when a sample has to change; the *committed bytes* are the
fixture. Nothing in `cargo test` runs this script or Pillow — the point of
committing it is that every sample in this directory can be audited and
regenerated rather than taken on trust.

Every image is built from fixed pixels, never from randomness, so two runs of
this script with the same Pillow version produce the same file.

Sizes are deliberately not all multiples of four, because a decoder that gets
the row stride wrong still succeeds on 8x8 and fails on 7x5:

    odd.png        7 x 5   PNG   lossless, top-left asserted
    odd.bmp        9 x 4   BMP   24bpp, row padded to 4 bytes, top-left asserted
    small.gif      5 x 3   GIF   palette, top-left asserted
    block.jpg      8 x 8   JPEG  lossy, dimensions only
    unicode ….png  3 x 3   PNG   non-ASCII file name
    truncated.png  —       PNG   cut mid-IDAT, must not decode
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
