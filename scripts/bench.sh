#!/bin/bash
# Regenerates the README speed table. Every number in the README comes from here,
# and a command that fails produces no number at all.
#
#   scripts/bench.sh [input.pdf]      default: RFC 9110 merged with itself five times
#
# Needs qpdf and mutool for the comparison columns, and poppler-utils for pdfinfo.
set -u
cd "$(dirname "$0")/.." || exit 1
PS=${PS:-target/release/pdfsurgeon}
[ -x "$PS" ] || { echo "build it first: cargo build --release" >&2; exit 1; }
for tool in qpdf mutool pdfinfo; do
  command -v "$tool" >/dev/null || { echo "needs $tool for the comparison columns" >&2; exit 1; }
done

# /tmp, not TMPDIR: qpdf cannot open files under the agent's scratch tree on this box.
TMP=$(mktemp -d /tmp/pdfsurgeon-bench.XXXXXX) || exit 1
trap 'rm -rf "$TMP"' EXIT

SRC=${1:-}
if [ -z "$SRC" ]; then
  SRC=$TMP/big.pdf
  curl -sL -m 180 -o "$TMP/rfc.pdf" https://www.rfc-editor.org/rfc/rfc9110.pdf || exit 1
  "$PS" merge -o "$SRC" "$TMP/rfc.pdf" "$TMP/rfc.pdf" "$TMP/rfc.pdf" "$TMP/rfc.pdf" "$TMP/rfc.pdf" >/dev/null || exit 1
fi
PAGES=$(pdfinfo "$SRC" | awk '/^Pages/ {print $2}')
echo "document: $(basename "$SRC"), $PAGES pages, $(du -h "$SRC" | cut -f1)"
echo "host:     $(nproc) cores, $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | sed 's/^ //')"
echo "tools:    pdfsurgeon $("$PS" --version | awk '{print $2}'), $(qpdf --version | head -1), mutool $((mutool -v 2>&1 | sed 's/.*version //') | head -1)"
echo "date:     $(date -u +%Y-%m-%d)"
echo

# Three runs, median in milliseconds. A non-zero exit is fatal: timing a command
# that failed would report a lie.
timed () {
  local start end rc
  local -a times=()
  for _ in 1 2 3; do
    start=$(date +%s%N)
    "$@" >/dev/null 2>&1
    rc=$?
    end=$(date +%s%N)
    if [ $rc -ne 0 ]; then echo "FAILED ($rc): $*" >&2; return 1; fi
    times+=( $(( (end - start) / 1000000 )) )
  done
  printf '%s\n' "${times[@]}" | sort -n | sed -n '2p'
}
t () { printf '%d.%02d s' $(( $1 / 1000 )) $(( ($1 % 1000) / 10 )); }

echo "| Operation | pdfsurgeon | qpdf | mutool |"
echo "|---|---|---|---|"

split_ps ()   { rm -rf "$TMP/s1"; "$PS" split "$SRC" --out "$TMP/s1"; }
split_qpdf () { rm -rf "$TMP/s2"; mkdir -p "$TMP/s2"; qpdf --split-pages=1 "$SRC" "$TMP/s2/p-%03d.pdf"; }
a=$(timed split_ps) || exit 1
b=$(timed split_qpdf) || exit 1
n=$(ls "$TMP/s1" | wc -l)
[ "$n" -eq "$PAGES" ] || { echo "split wrote $n files, expected $PAGES" >&2; exit 1; }
printf '| Split every page into its own file | %s | %s | not supported |\n' "$(t "$a")" "$(t "$b")"

merge_ps     () { "$PS" merge -o "$TMP/m1.pdf" "$SRC" "$SRC"; }
merge_qpdf   () { qpdf --empty --pages "$SRC" 1-z "$SRC" 1-z -- "$TMP/m2.pdf"; }
merge_mutool () { mutool merge -o "$TMP/m3.pdf" "$SRC" "$SRC"; }
m1=$(timed merge_ps) || exit 1
m2=$(timed merge_qpdf) || exit 1
m3=$(timed merge_mutool) || exit 1
for f in m1 m2 m3; do
  got=$(pdfinfo "$TMP/$f.pdf" | awk '/^Pages/ {print $2}')
  [ "$got" -eq "$((PAGES * 2))" ] || { echo "$f.pdf has $got pages, expected $((PAGES * 2))" >&2; exit 1; }
done
printf '| Merge two copies (%d pages) | %s | %s | %s |\n' "$((PAGES * 2))" "$(t "$m1")" "$(t "$m2")" "$(t "$m3")"

sel_ps     () { "$PS" select "$SRC" --pages 1,5,10 -o "$TMP/x1.pdf"; }
sel_qpdf   () { qpdf --empty --pages "$SRC" 1,5,10 -- "$TMP/x2.pdf"; }
sel_mutool () { mutool merge -o "$TMP/x3.pdf" "$SRC" 1,5,10; }
s1=$(timed sel_ps) || exit 1
s2=$(timed sel_qpdf) || exit 1
s3=$(timed sel_mutool) || exit 1
for f in x1 x2 x3; do
  got=$(pdfinfo "$TMP/$f.pdf" | awk '/^Pages/ {print $2}')
  [ "$got" -eq 3 ] || { echo "$f.pdf has $got pages, expected 3" >&2; exit 1; }
done
printf '| Select 3 pages | %s | %s | %s |\n' "$(t "$s1")" "$(t "$s2")" "$(t "$s3")"

# mutool 1.27 has no per-page rotate, so that cell says so rather than being wrong.
r1=$(timed "$PS" rotate "$SRC" --pages "1-$PAGES" --angle 90 -o "$TMP/r1.pdf") || exit 1
r2=$(timed qpdf --rotate=+90:1-z "$SRC" "$TMP/r2.pdf") || exit 1
printf '| Rotate every page (mutool has no per-page rotate) | %s | %s | not supported |\n' "$(t "$r1")" "$(t "$r2")"
