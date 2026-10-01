# Generate the Phase 7 playback corpus.
#
# Run by hand when a sample has to change; the *committed bytes* are the
# fixture — the same rule `tests/corpus/image/generate.py` states. Nothing in
# `cargo test` runs this script or ffmpeg: the point of committing it is that
# every sample in this directory can be audited and regenerated rather than
# taken on trust.
#
# Requires ffmpeg on PATH (generation only). Every input is a synthesised
# lavfi source, so two runs with the same ffmpeg build produce the same
# bytes: no file from anywhere else is copied in, and nothing here carries
# someone else's copyright. The exact bytes still depend on the encoder
# versions, so the version used for the committed set is recorded below.
#
# One sample per format §18.4 lists, all from the same one-second tone and
# the same 160x120 test picture, so a failure in one format can be compared
# against a success in another without the content also changing:
#
#     tone.wav     44100 Hz 16-bit stereo PCM      (audio only)
#     tone.mp3     LAME                           (audio only)
#     tone.flac    FLAC                           (audio only)
#     tone.mp4     H.264 + AAC                    (video)
#     tone.mkv     H.264 + AAC                    (video)
#     tone.webm    VP9 + Opus                     (video)
#     tone.avi     MPEG-4 + MP3                   (video)
#     tone.mov     H.264 + AAC                    (video)
#     truncated.mp4  the mp4 cut inside its header (must not decode)
#
# `truncated.mp4` exists so error mapping has something real to fail on —
# the lesson `image/truncated.png` taught in M7: a failure path with no
# fixture is a failure path nobody has ever run.

set -eu

cd "$(dirname "$0")"

tone="sine=frequency=440:sample_rate=44100:duration=1"
picture="testsrc2=size=160x120:rate=10:duration=1"

ffmpeg -version | head -n 1

# Audio only: the tone, one file per container. (ffmpeg version for the committed set: N-126947-g45f3fecca9-20260928.)
ffmpeg -y -v error -f lavfi -i "$tone" -ac 2 tone.wav
ffmpeg -y -v error -f lavfi -i "$tone" -ac 2 -c:a libmp3lame tone.mp3
ffmpeg -y -v error -f lavfi -i "$tone" -ac 2 -c:a flac tone.flac

# Video: the picture over the tone.
ffmpeg -y -v error -f lavfi -i "$picture" -f lavfi -i "$tone" \
    -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest tone.mp4
ffmpeg -y -v error -f lavfi -i "$picture" -f lavfi -i "$tone" \
    -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest tone.mkv
ffmpeg -y -v error -f lavfi -i "$picture" -f lavfi -i "$tone" \
    -c:v libvpx-vp9 -c:a libopus -shortest tone.webm
ffmpeg -y -v error -f lavfi -i "$picture" -f lavfi -i "$tone" \
    -c:v mpeg4 -c:a libmp3lame -shortest tone.avi
ffmpeg -y -v error -f lavfi -i "$picture" -f lavfi -i "$tone" \
    -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest tone.mov

# Cut inside the header, not at the tail: a file that merely stops early is
# one some decoders half-accept, while a header that never finishes is
# refused by all of them (the same reasoning as image/truncated.png).
whole=$(wc -c < tone.mp4)
head -c $(( whole / 3 )) tone.mp4 > truncated.mp4

ls -la
