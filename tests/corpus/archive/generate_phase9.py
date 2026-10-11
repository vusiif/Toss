"""Generate the Phase 9 corpus samples (tests/corpus/archive).

Run from the repository root:  python tests/corpus/archive/generate_phase9.py
Every sample is committed as bytes afterwards; tests never run this file
(image corpus follows the same rule with generate.py).

Samples produced:
  security/hostile-windows-names.zip  Windows destination-semantics attack
                                      (device names, ADS colon, trailing
                                      dot/space, backslash traversal) with an
                                      ordinary control member first.
  security/windows-superscript-devices.zip
                                      COM\u00b9.txt / LPT\u00b2 members — the superscript
                                      (U+00B9/U+00B2) corner of the official
                                      Windows reserved-name list.
  security/unc-path.zip               UNC-rooted member.
  corrupt/size-lie.zip                declared uncompressed size far smaller
                                      than the real inflated stream.
  unicode/utf8-flag-paths.zip         UTF-8 flag (bit 11) names: CJK, emoji,
                                      combining mark, ZWJ, RLO override, ZWSP.
"""
import os
import struct
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))


def write(name: str, members: list[tuple[str, bytes]], compress=zipfile.ZIP_DEFLATED) -> None:
    path = os.path.join(HERE, *name.split("/"))
    with zipfile.ZipFile(path, "w", compress) as zf:
        for member, data in members:
            zf.writestr(member, data)
    print(f"wrote {name} ({os.path.getsize(path)} B, {len(members)} members)")


def hostile_windows_names() -> None:
    members = [
        ("ok.txt", b"ok.txt"),          # ordinary control member, written first
        ("NUL", b"device"),             # null device
        ("NUL.txt", b"device"),         # device name with an extension
        ("CON", b"device"),             # console device
        ("foo.txt:stream", b"ads"),     # alternate data stream
        ("bar.txt ", b"trimmed"),       # trailing space
        ("baz.txt.", b"trimmed"),       # trailing dot
        ("..\\..\\evil.txt", b"escape"),  # backslash traversal
    ]
    write("security/hostile-windows-names.zip", members, zipfile.ZIP_STORED)


def unc_path() -> None:
    write(
        "security/unc-path.zip",
        [("//server/share/toss-unc-should-not-exist", b"unc")],
    )


def size_lie() -> None:
    """A deflated member whose recorded uncompressed size is a lie.

    zipfile writes honest sizes, so the two size fields (local header and
    central directory) are patched after the fact; the deflate stream itself
    is untouched and really expands to the full payload.
    """
    payload = b"size-lie payload\n" * 64  # 1088 real bytes
    path = os.path.join(HERE, "corrupt", "size-lie.zip")
    fake = 16  # recorded uncompressed size

    buf = __import__("io").BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("liar.txt", payload)
    data = bytearray(buf.getvalue())

    # Local file header: after the 4-byte signature and the fixed field
    # block, compressed size sits at 18 and uncompressed size at 22.
    assert struct.unpack_from("<I", data, 0)[0] == 0x04034B50
    compressed = struct.unpack_from("<I", data, 18)[0]
    struct.pack_into("<I", data, 22, fake)

    # Central directory: same fields at offset 24/28 of the record.
    central = data.rfind(b"PK\x01\x02")
    assert central > 0
    struct.pack_into("<I", data, central + 24, fake)
    # Keep compressed size consistent with the local header.
    struct.pack_into("<I", data, central + 20, compressed)

    with open(path, "wb") as fh:
        fh.write(data)
    print(f"wrote corrupt/size-lie.zip ({len(data)} B, real usize {len(payload)}, fake {fake})")


def utf8_flag_paths() -> None:
    members = [
        ("文件夹/中文 文件.txt", "内容\n".encode()),
        ("emoji-🎉.txt", b"party\n"),
        ("combining-cafe\u0301.txt", "cafe with acute\n".encode()),
        ("zwj-\u200d.txt", b"zero width joiner\n"),
        # RLO override with a benign extension on purpose: this machine's
        # antivirus quarantines a *zip* whose member name is RLO-obfuscated
        # toward `.exe` within seconds of the file appearing (observed while
        # building this corpus — see docs/RELEASE_READINESS_REPORT.md), so
        # the codepoint is covered without smuggling a shape AV deletes.
        ("rlo-\u202etxt.txt", "name visually reversed\n".encode()),
        ("zwsp-\u200b.txt", b"zero width space\n"),
    ]
    write("unicode/utf8-flag-paths.zip", members)


def windows_superscript_devices() -> None:
    """The superscript corner of Microsoft's reserved-name list.

    COM¹ / LPT² use ISO/IEC 8859-1 superscript digits (U+00B9 / U+00B2),
    which Windows treats as digits in COM#/LPT# device names — the
    official list names them explicitly:
    https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file
    One member carries an extension (the doc reserves the extension forms
    too). Refusal must happen in Toss's path policy *before* any open —
    no test opens a real COM/LPT device.
    """
    members = [
        ("ok.txt", b"ok.txt"),     # ordinary control member, written first
        ("COM\u00b9.txt", b"device"),  # superscript-one, with extension
        ("LPT\u00b2", b"device"),      # superscript-two, bare
    ]
    write("security/windows-superscript-devices.zip", members, zipfile.ZIP_STORED)


if __name__ == "__main__":
    hostile_windows_names()
    unc_path()
    size_lie()
    utf8_flag_paths()
    windows_superscript_devices()
