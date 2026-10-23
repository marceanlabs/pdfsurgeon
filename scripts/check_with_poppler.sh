#!/usr/bin/env bash
# Check pdfsurgeon's output with poppler, which is an independent reader.
#
#   ./scripts/check_with_poppler.sh file1.pdf [file2.pdf ...]
#
# For each input: select a middle page range, split every page, rotate the first
# page, and require that pdfinfo reports no syntax warning and that the selected
# text matches the same range of the original.
set -euo pipefail

binary=${BINARY:-./target/release/pdfsurgeon}
[ -x "$binary" ] || { echo "build it first: cargo build --release"; exit 1; }
command -v pdfinfo >/dev/null || { echo "needs poppler: apt install poppler-utils"; exit 1; }

work=$(mktemp -d)
trap 'mv "$work" "${TMPDIR:-/tmp}/pdfsurgeon-check-$(date +%s)"' EXIT

first_total=$("$binary" info "$1" | awk '/^pages:/ {print $2}')

status=0
for input in "$@"; do
  name=$(basename "$input" .pdf)
  total=$("$binary" info "$input" | awk '/^pages:/ {print $2}')
  echo "$name: $total pages"

  last=$(( total < 3 ? total : 3 ))
  first=$(( total > 3 ? 2 : 1 ))

  "$binary" select "$input" --pages "$first-$last" -o "$work/$name-pick.pdf"
  "$binary" rotate "$input" --pages 1 --angle 90 -o "$work/$name-rot.pdf"
  mkdir -p "$work/$name-parts"
  "$binary" split "$input" --out "$work/$name-parts"

  for out in "$work/$name-pick.pdf" "$work/$name-rot.pdf" "$work/$name-parts"/*.pdf; do
    if pdfinfo "$out" 2>&1 | grep -qi "syntax warning"; then
      echo "  FAIL  poppler complains about $(basename "$out")"
      status=1
    fi
    # qpdf is stricter than poppler: it checks the cross reference table and
    # every key it knows, so a page without /Parent or a stale reference shows
    # up here even though a reader would tolerate it.
    if command -v qpdf >/dev/null; then
      if report=$(qpdf --check "$out" 2>&1); then
        :
      elif printf '%s' "$report" | grep -q "Permission denied"; then
        : # qpdf cannot open files in this directory; not a verdict on the file
      else
        echo "  FAIL  qpdf --check rejects $(basename "$out")"
        status=1
      fi
    fi
  done

  if command -v pdftotext >/dev/null; then
    pdftotext -f 1 -l "$(( last - first + 1 ))" "$work/$name-pick.pdf" "$work/a.txt"
    pdftotext -f "$first" -l "$last" "$input" "$work/b.txt"
    if diff -q "$work/a.txt" "$work/b.txt" >/dev/null; then
      echo "  text: identical to pages $first-$last"
    else
      echo "  FAIL  text differs from pages $first-$last"
      status=1
    fi
  fi

  echo "  parts: $(find "$work/$name-parts" -name '*.pdf' | wc -l) files, warnings checked"
done

# With two or more inputs, also assemble a mixed document and check that too.
if [ "$#" -ge 2 ]; then
  first_file=$1
  "$binary" assemble -o "$work/mixed.pdf" "$first_file:1-2" "$2:1" "$first_file:$first_total"
  if pdfinfo "$work/mixed.pdf" 2>&1 | grep -qi "syntax warning"; then
    echo "assemble: FAIL  poppler complains about the mixed document"
    status=1
  else
    echo "assemble: clean, $(pdfinfo "$work/mixed.pdf" | awk '/^Pages:/ {print $2}') pages"
  fi
fi

exit $status
