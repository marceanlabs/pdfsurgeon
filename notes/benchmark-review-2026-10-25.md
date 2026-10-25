# Benchmark review, 25 October 2026

Two things came out of re-measuring for the README, and both matter more than the
table.

## The headline claim is false under qpdf

The README says "the output does not make other PDF tools warn". Measured today on
a 970 page document built from RFC 9110 merged five times:

```
$ pdfsurgeon merge -o out.pdf big.pdf big.pdf
$ qpdf --check out.pdf
WARNING: Name/Number tree node (object 125772): attempting to repair after error:
         keys are not sorted in validate
qpdf: operation succeeded with warnings
```

Select, split and rotate outputs do the same, each naming a name tree node. The
source document is clean (`qpdf --check rfc.pdf` says nothing) and so is qpdf's own
extraction of the same page, so this is introduced here, not inherited. Poppler
does not warn, which is why the existing verification missed it: every check in
`tests/` and in the README uses `pdfinfo`, and `pdfinfo` is silent on this.

## Split output is 34 times larger than it needs to be

| Output | pdfsurgeon | qpdf |
|---|---|---|
| Every page as its own file, 970 pages | 1.3 GB | 38 MB |
| One page file | 1.3 MB | 35 KB |

Both files carry 30 font references and a similar object count, so the size is not
objects; something reachable from the page is being copied whole. A user splitting
a 970 page file gets 1.3 GB instead of 38 MB, and the write time that follows is
most of why our split is now slower than qpdf's.

## Fresh numbers, for the record

970 pages, 13 MB, median of three, same machine, measured 25 Oct 2026:

| Operation | pdfsurgeon | qpdf | mutool |
|---|---|---|---|
| Split every page | 4.31 s to 5.97 s across runs | 3.59 s to 3.65 s | not supported |
| Merge two copies (1940 pages) | 1.06 s | 5.28 s | 2.52 s |
| Select 3 pages | 0.33 s | 0.32 s | 0.09 s |
| Rotate every page | 0.53 s | 3.10 s | not supported |

The old README numbers (0.89 s split against qpdf's 4.81 s, and 2.27 s for a three
page select) do not reproduce on this document at all. Whatever produced them,
they cannot be published again.

The split row is disk-bound and moves between 4.3 s and 6.0 s on the same machine.
A single number understates that; the README carries the size defect instead,
which does not move.

A run whose qpdf column reads 0.01 s to 0.02 s is a failed run, not a fast one:
qpdf could not open the input under the agent's scratch tree and exited in
milliseconds. `scripts/bench.sh` now works in `/tmp` and treats a non-zero exit as
fatal, so that cannot be published again.

## Order of work

1. Fix the name tree ordering. It is the pitch, and right now a second opinion tool
   proves the pitch wrong.
2. Fix what makes a single page 1.3 MB.
3. Re-measure, then rewrite the Speed section with these numbers and the script
   that produces them (`scripts/bench.sh`).
