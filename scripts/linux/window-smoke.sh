#!/usr/bin/env bash
# Open the real viewer on a virtual X display and check the window exists,
# is titled after the picture and has the size the shared rule asks for.
# Needs Xvfb, xwininfo and the X libraries a desktop always has
# (Debian/Ubuntu: xvfb x11-utils libxcursor1 libxkbcommon-x11-0).
#
# usage: window-smoke.sh <toss-binary> <repo-root>
set -uo pipefail

toss=$(readlink -f "${1:?usage: window-smoke.sh <toss> <repo>}")
repo=$(readlink -f "${2:?usage: window-smoke.sh <toss> <repo>}")
work=$(mktemp -d)
cleanup() { kill "${viewer:-}" "${xvfb:-}" 2>/dev/null; rm -rf "$work"; }
trap cleanup EXIT

export DISPLAY=:97
unset WAYLAND_DISPLAY WAYLAND_SOCKET
Xvfb "$DISPLAY" -screen 0 1600x1000x24 -nolisten tcp >/dev/null 2>&1 &
xvfb=$!
for _ in $(seq 50); do xwininfo -root >/dev/null 2>&1 && break; sleep 0.1; done

fail=0
open_and_check() { # open_and_check <file> <title> <expected WxH>
  local file=$1 title=$2 size=$3
  "$toss" view "$file" >"$work/out" 2>"$work/err" </dev/null &
  viewer=$!
  local tree=""
  for _ in $(seq 50); do
    tree=$(xwininfo -root -tree 2>/dev/null | grep -F "\"$title\"" || true)
    [[ -n $tree ]] && break
    kill -0 "$viewer" 2>/dev/null || break
    sleep 0.1
  done
  if [[ -z $tree ]]; then
    echo "FAIL $title: no window appeared"; sed 's/^/     | /' "$work/err"; fail=1
  elif [[ $tree != *" ${size}+"* ]]; then
    echo "FAIL $title: window is not $size: $tree"; fail=1
  else
    echo "ok   $title opened at $size"
  fi
  kill -0 "$viewer" 2>/dev/null || { echo "FAIL $title: viewer exited early"; fail=1; }
  kill "$viewer" 2>/dev/null; wait "$viewer" 2>/dev/null
  grep -q 'Toss' "$work/out" || { echo "FAIL $title: no console summary"; fail=1; }
}

# 320×200 fits inside half the screen (800×500): opens at its own size.
open_and_check "$repo/tests/corpus/image/panel.png" panel.png 320x200
# 7×5 opens at the 320×200 minimum client size.
open_and_check "$repo/tests/corpus/image/odd.png" odd.png 320x200

play_and_check() { # play_and_check <file> <expected WxH>
  local file=$1 size=$2 title
  title=$(basename "$file")
  local start=$SECONDS
  "$toss" play "$file" >"$work/out" 2>"$work/err" </dev/null &
  viewer=$!
  local tree=""
  for _ in $(seq 50); do
    tree=$(xwininfo -root -tree 2>/dev/null | grep -F "\"$title\"" || true)
    [[ -n $tree ]] && break
    kill -0 "$viewer" 2>/dev/null || break
    sleep 0.1
  done
  if [[ -z $tree ]]; then
    echo "FAIL $title: no window appeared"; sed 's/^/     | /' "$work/err"; fail=1
  elif [[ $tree != *" ${size}+"* ]]; then
    echo "FAIL $title: window is not $size: $tree"; fail=1
  else
    echo "ok   $title opened at $size"
  fi
  # A one-second film closes its own window when it ends.
  local code=""
  for _ in $(seq 100); do
    kill -0 "$viewer" 2>/dev/null || { wait "$viewer"; code=$?; break; }
    sleep 0.1
  done
  if [[ -z $code ]]; then
    echo "FAIL $title: still playing after 10 s"; kill "$viewer" 2>/dev/null; fail=1
  elif [[ $code != 0 ]]; then
    echo "FAIL $title: exit $code"; sed 's/^/     | /' "$work/err"; fail=1
  else
    echo "ok   $title played to its end in $((SECONDS - start)) s"
  fi
  grep -q 'Media Player' "$work/out" || { echo "FAIL $title: no console summary"; fail=1; }
}

# 160×120 films open at the minimum, keeping their 4:3 shape.
play_and_check "$repo/tests/corpus/media/tone.mp4" 320x240
play_and_check "$repo/tests/corpus/media/hevc.mkv" 320x240

exit $fail
