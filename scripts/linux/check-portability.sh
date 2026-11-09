#!/usr/bin/env bash
# Refuse a Linux `toss` binary that would not start on every supported distro.
#
# Two rules, both checked on the ELF itself rather than trusted:
#   1. It may hard-link only the glibc core and the GCC unwinder. Anything
#      optional (X11, Wayland, ALSA, ...) is loaded at run time, so a machine
#      without it still starts Toss and loses only that capability.
#   2. No symbol may need a glibc newer than the supported floor.
#
# usage: check-portability.sh <binary> [max-glibc]   (default floor: 2.28)
set -euo pipefail

bin=${1:?usage: check-portability.sh <binary> [max-glibc]}
floor=${2:-2.28}

allowed=(libc.so.6 libm.so.6 libgcc_s.so.1 ld-linux-x86-64.so.2
         ld-linux-aarch64.so.1 libdl.so.2 libpthread.so.0 librt.so.1)

status=0

mapfile -t needed < <(readelf -d "$bin" | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p')
for lib in "${needed[@]}"; do
  ok=0
  for a in "${allowed[@]}"; do [[ $lib == "$a" ]] && ok=1; done
  if (( ok )); then
    echo "  linked: $lib"
  else
    echo "::error::$bin hard-links $lib; load it at run time instead" >&2
    status=1
  fi
done

newest=$(objdump -T "$bin" | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/GLIBC_//' | sort -Vu | tail -n1)
echo "  newest glibc symbol: ${newest:-none} (floor $floor)"
if [[ -n $newest ]] && [[ $(printf '%s\n%s\n' "$floor" "$newest" | sort -V | tail -n1) != "$floor" ]]; then
  echo "::error::$bin needs GLIBC_$newest, newer than the supported $floor" >&2
  objdump -T "$bin" | grep "GLIBC_$newest" | head -n 20 >&2
  status=1
fi

interp=$(readelf -l "$bin" | sed -n 's/.*Requesting program interpreter: \(.*\)\]/\1/p')
echo "  interpreter: ${interp:-none}"

exit $status
