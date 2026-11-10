#!/usr/bin/env bash
# Exercise one Linux `toss` binary the way a user would, on whatever distro
# this runs on. Needs only bash and coreutils, so it runs in minimal images.
#
# usage: smoke.sh <toss-binary> <repo-root>
# Capabilities Linux does not have yet are asserted as exit 3, so this file
# changes (deliberately) when each one lands.
set -uo pipefail

toss=$(readlink -f "${1:?usage: smoke.sh <toss> <repo>}")
repo=$(readlink -f "${2:?usage: smoke.sh <toss> <repo>}")
corpus=$repo/tests/corpus
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work" || exit 1

pass=0 fail=0
check() { # check <name> <expected-exit> <command...>
  local name=$1 want=$2; shift 2
  "$@" >"$work/out" 2>"$work/err" </dev/null
  local got=$?
  if [[ $got == "$want" ]]; then
    pass=$((pass + 1)); echo "ok   $name"
  else
    fail=$((fail + 1)); echo "FAIL $name: exit $got, wanted $want"
    sed 's/^/     | /' "$work/err" | head -n 5
  fi
}
expect() { # expect <name> <condition...>
  if "${@:2}"; then pass=$((pass + 1)); echo "ok   $1"
  else fail=$((fail + 1)); echo "FAIL $1"; fi
}

. /etc/os-release 2>/dev/null
echo "== ${PRETTY_NAME:-unknown distro}, $(ldd --version 2>&1 | head -n1)"

check "no arguments is invalid"        2 "$toss"
expect "  ...and says so on stderr"      grep -q '^Error: ' "$work/err"
check "help"                           0 "$toss" --help
check "missing input"                  4 "$toss" "$work/nope.zip"

cp "$corpus/archive/valid/simple.zip" "$corpus/archive/security/traversal.zip" .
check "info"                           0 "$toss" info simple.zip
check "extract zip"                    0 "$toss" extract simple.zip
expect "  ...output directory exists"    test -d simple
check "extract refuses to overwrite"   1 "$toss" extract simple.zip
# 7z and rar5 samples unpack to the same name as the zip: separate folders.
for sample in simple.7z simple-rar5.rar; do
  mkdir "d-$sample" && cp "$corpus/archive/valid/$sample" "d-$sample/"
  check "extract ${sample##*.}"        0 "$toss" extract "d-$sample/$sample"
done
check "traversal is refused"           1 "$toss" extract traversal.zip
expect "  ...nothing escaped"            test ! -e ../evil.txt -a ! -e traversal
mkdir -p packme/sub && echo hello >packme/sub/a.txt
check "pack a directory"               0 "$toss" pack packme
expect "  ...archive written"            compgen -G 'packme.*' >/dev/null

printf 'name,qty\n甲,007\nb,2\n' >table.csv
check "csv to redirected stdout"       0 "$toss" table.csv
expect "  ...keeps the data verbatim"    grep -q '甲,007' "$work/out"
cp "$repo/tests/fixtures/word-legacy/nested_tables.doc" .
check "legacy doc to redirected stdout" 0 "$toss" nested_tables.doc
expect "  ...produces text"              test -s "$work/out"

two=("$corpus/archive/corrupt/truncated.zip" table.csv)
check "batch continues after a failure" 6 "$toss" "${two[@]}"
expect "  ...summary on stderr"          grep -q 'batch complete' "$work/err"

# Pictures and PDFs decode before a display is looked for, so a broken
# file is exit 6 everywhere; a good one with no display and no terminal
# (this script's stdio is redirected) is refused in words with exit 3.
headless=(env -u DISPLAY -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u XDG_RUNTIME_DIR)
check "broken picture is corrupt"      6 "${headless[@]}" "$toss" view "$corpus/image/truncated.png"
check "picture with nowhere to show"   3 "${headless[@]}" "$toss" view "$corpus/image/odd.png"
expect "  ...says why"                   grep -q 'no display' "$work/err"
printf 'not a pdf' >bad.pdf
check "broken PDF is corrupt"          6 "${headless[@]}" "$toss" view bad.pdf

# Not on Linux yet: becomes a real check when its phase lands.
# (`hash` is unimplemented on every platform, so it is not asserted here.)
check "media playback (pending)"       3 "$toss" play "$corpus/media/tone.wav"

echo "== $pass passed, $fail failed"
(( fail == 0 ))
