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

# Under `target/`, not the ambient TMPDIR: qpdf 12 on this machine refuses to
# create files under some sandboxed temp paths ("Permission denied") and the
# oracle needs to write the reference document beside our output.
mkdir -p target
work=$(mktemp -d target/oracle.XXXXXX)
status=0
cleanup() {
  if [ "$status" -eq 0 ]; then rm -rf "$work"; else echo "kept for inspection: $work"; fi
}
trap cleanup EXIT

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
      # `case`, not `grep -q`: with pipefail a match can kill qpdf with SIGPIPE
      # and turn a pass into a failure.
      report=$(qpdf --check "$out" 2>&1) || case "$report" in
        *"Permission denied"*) : ;; # qpdf cannot open this directory, not a verdict
        *) echo "  FAIL  qpdf --check rejects $(basename "$out")"; status=1 ;;
      esac
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

  # An encrypted copy of the same file, made by qpdf, has to open with the
  # empty password and read back the same text.
  if command -v qpdf >/dev/null && command -v pdftotext >/dev/null; then
    qpdf --encrypt '' '' 256 -- "$input" "$work/$name-locked.pdf"
    "$binary" select "$work/$name-locked.pdf" --pages 1 -o "$work/$name-locked-pick.pdf"
    pdftotext "$work/$name-locked-pick.pdf" "$work/locked.txt"
    pdftotext -f 1 -l 1 "$input" "$work/plain.txt"
    if diff -q "$work/locked.txt" "$work/plain.txt" >/dev/null; then
      echo "  encrypted copy: opens, page one reads the same"
    else
      echo "  FAIL  encrypted copy: page one reads differently"
      status=1
    fi
    if pdfinfo "$work/$name-locked-pick.pdf" | grep -q '^Encrypted:.*no'; then
      echo "  encrypted copy: the output is not encrypted"
    else
      echo "  FAIL  encrypted copy: the output is still encrypted"
      status=1
    fi
  fi

  # A page spec the command has to expand, with a reference built by qpdf
  # rather than by us: the text of the selection must match the text of the
  # same pages in the reference document.
  if command -v qpdf >/dev/null && command -v pdftotext >/dev/null && [ "$total" -ge 5 ]; then
    for spec in "1,3,5" "2-4" "$(( total - 1 ))-$total" "4"; do
      "$binary" select "$input" --pages "$spec" -o "$work/$name-spec.pdf"
      qpdf "$input" --pages . "$spec" -- "$work/$name-ref.pdf"
      pdftotext "$work/$name-spec.pdf" "$work/spec.txt"
      pdftotext "$work/$name-ref.pdf" "$work/ref.txt"
      if diff -q "$work/spec.txt" "$work/ref.txt" >/dev/null; then
        echo "  select $spec: text matches qpdf's copy"
      else
        echo "  FAIL  select $spec: text differs from qpdf's copy"
        status=1
      fi
    done
  fi
done

# With two or more inputs, also assemble and merge mixed documents and check
# both: merge is the operation that has to splice page trees and outlines.
if [ "$#" -ge 2 ]; then
  first_file=$1
  "$binary" assemble -o "$work/mixed.pdf" "$first_file:1-2" "$2:1" "$first_file:$first_total"
  if pdfinfo "$work/mixed.pdf" 2>&1 | grep -qi "syntax warning"; then
    echo "assemble: FAIL  poppler complains about the mixed document"
    status=1
  else
    echo "assemble: clean, $(pdfinfo "$work/mixed.pdf" | awk '/^Pages:/ {print $2}') pages"
  fi

  "$binary" merge -o "$work/merged.pdf" "$1" "$2"
  if pdfinfo "$work/merged.pdf" 2>&1 | grep -qi "syntax warning"; then
    echo "merge: FAIL  poppler complains about the merged document"
    status=1
  elif command -v qpdf >/dev/null; then
    report=$(qpdf --check "$work/merged.pdf" 2>&1) || case "$report" in
      *"Permission denied"*) : ;;
      *) echo "merge: FAIL  qpdf --check rejects the merged document"; status=1 ;;
    esac
    echo "merge: $(pdfinfo "$work/merged.pdf" | awk '/^Pages:/ {print $2}') pages, checked"
  else
    echo "merge: clean, $(pdfinfo "$work/merged.pdf" | awk '/^Pages:/ {print $2}') pages"
  fi
fi

exit $status
