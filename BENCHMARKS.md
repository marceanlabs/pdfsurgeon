# pdfsurgeon benchmarks

970 pages, 13 MB, median of three runs, against qpdf 12.3.2 and mutool 1.27.
`scripts/bench.sh` rebuilds the document and reproduces the table.

| Operation | pdfsurgeon | qpdf | mutool |
|---|---|---|---|
| Split every page | 1.02 s, 36 MB | 3.71 s, 38 MB | not supported |
| Merge 1,940 pages | 0.75 s | 5.36 s | 2.60 s |
| Select 3 pages | 0.23 s, 77 KB | 0.26 s, 90 KB | 0.11 s, 35 KB |
| Rotate every page | 0.38 s | 3.13 s | not supported |

## Why mutool wins the select row

Selecting three pages is the one row where mutool is quicker, and the reason is
worth knowing.

mutool reads pages as a stream and never builds a picture of the whole document.
pdfsurgeon builds one, because that is what lets it fix the links, bookmarks and
form fields afterwards. The cost here is a fifth of a second, and it is what makes
the output clean.

## Reproducing it

```console
$ scripts/bench.sh
```
