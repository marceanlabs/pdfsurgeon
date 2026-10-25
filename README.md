# pdfsurgeon

[![crates.io](https://img.shields.io/crates/v/pdfsurgeon.svg)](https://crates.io/crates/pdfsurgeon)
[![ci](https://github.com/marceanlabs/pdfsurgeon/actions/workflows/ci.yml/badge.svg)](https://github.com/marceanlabs/pdfsurgeon/actions/workflows/ci.yml)

Page level surgery for PDF files: merge, split, select and rotate, and leave the
rest of the document alone. Page removal does not leave dangling destinations
behind, and `qpdf --check` reports no warning on the output of any command.

## Install

```
cargo install pdfsurgeon
```

Or a prebuilt binary for Linux, macOS or Windows from the
[releases page](https://github.com/marceanlabs/pdfsurgeon/releases).

## Commands

```
pdfsurgeon info <file>
pdfsurgeon merge -o <out.pdf> <in1.pdf> <in2.pdf> ...
pdfsurgeon assemble -o <out.pdf> <file[:pages]> ...
pdfsurgeon split <file> [--out <dir>]
pdfsurgeon select <file> --pages <spec> -o <out.pdf>
pdfsurgeon rotate <file> --pages <spec> --angle <deg> -o <out.pdf>
```

Page specs take a page, a range or a list of either: `1`, `1-4`, `1,3-5,9`.
`--password` opens an encrypted file. `assemble` takes pages from many files in
the order you list them, which is the general case of `merge`:

```
pdfsurgeon assemble -o bid.pdf cover.pdf report.pdf:2-4 appendix.pdf
```

## Speed

970 pages, 13 MB, median of three, against qpdf 12.3.2 and mutool 1.27.
`scripts/bench.sh` rebuilds the document and reproduces the table.

| Operation | pdfsurgeon | qpdf | mutool |
|---|---|---|---|
| Split every page | 1.02 s, 36 MB | 3.71 s, 38 MB | not supported |
| Merge 1,940 pages | 0.75 s | 5.36 s | 2.60 s |
| Select 3 pages | 0.23 s, 77 KB | 0.26 s, 90 KB | 0.11 s, 35 KB |
| Rotate every page | 0.38 s | 3.13 s | not supported |

Select is the row it loses to mutool, and the reason is the parser rather than
this code: mutool streams pages instead of building an object model. This tool
pays that parse for what it does with the model, which is rebuilding the page
tree so that links, bookmarks and forms still point at something real.

## What it will not do

No repair of damaged files: an input that does not parse cleanly is refused
rather than guessed at. No encryption on output; an encrypted input is opened with
`--password` and comes out unencrypted. No recompression and no font work, so the
output keeps the input's size apart from the pages removed. Signatures are dropped
rather than left to fail, and field scripts are dropped because a merge can rename
their fields. A tagged PDF comes out untagged and its layers come out flat. An
attachment travels only when every page does, because otherwise the file would be
a false statement about the part.

## As a library

```rust
use pdfsurgeon::{Document, PageSpec};

let doc = Document::load("report.pdf")?;
let keep = PageSpec::parse("1-3,7", doc.get_pages().len() as u32)?;
pdfsurgeon::select(&doc, keep.pages())?.save("extract.pdf")?;
```

Every command is a public function, errors are one `Error` enum, and
`lopdf::Document` is re-exported so callers cannot sit on a different version.
Docs are at <https://docs.rs/pdfsurgeon>. 85 tests and a nightly fuzz run are in
the repo.

## Licence

Apache-2.0.
