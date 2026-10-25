# pdfsurgeon

Take pages out of a PDF, put PDFs together, or split one into many, and leave the
rest of the document alone.

```console
$ pdfsurgeon select report.pdf --pages 1-3,7 -o extract.pdf
$ pdfsurgeon merge -o bundle.pdf cover.pdf report.pdf
```

The hard part of editing a PDF is everything you cannot see. A file keeps internal
lists of links, bookmarks, form fields and destinations, and each entry points at a
page. Remove a page and those pointers stay where they were, aimed at nothing, so
the reader opens the file and complains about it.

pdfsurgeon rebuilds those lists as it goes, which is what makes the output clean.
`qpdf --check`, an independent tool that is strict about PDF structure, reports no
warning on the result of any command.

## Install

```console
$ cargo install pdfsurgeon
```

Prebuilt binaries for Linux, macOS and Windows are on the
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

A page spec is a page, a range, or a list of either: `1`, `1-4`, `1,3-5,9`.
`--password` opens an encrypted file.

`assemble` is `merge` with more control. It takes pages from several files in the
order you list them:

```console
$ pdfsurgeon assemble -o bid.pdf cover.pdf report.pdf:2-4 appendix.pdf
```

That builds a bid out of the cover, pages 2 to 4 of the report, and the appendix.

## How fast it is

970 pages, 13 MB, median of three runs, against qpdf 12.3.2 and mutool 1.27.
`scripts/bench.sh` rebuilds the document and reproduces the table.

| Operation | pdfsurgeon | qpdf | mutool |
|---|---|---|---|
| Split every page | 1.02 s, 36 MB | 3.71 s, 38 MB | not supported |
| Merge 1,940 pages | 0.75 s | 5.36 s | 2.60 s |
| Select 3 pages | 0.23 s, 77 KB | 0.26 s, 90 KB | 0.11 s, 35 KB |
| Rotate every page | 0.38 s | 3.13 s | not supported |

Selecting three pages is the one row where mutool is quicker, and the reason is
worth knowing. mutool reads pages as a stream and never builds a picture of the
whole document. pdfsurgeon builds one, because that is what lets it fix the links,
bookmarks and form fields afterwards. The cost here is a fifth of a second, and it
is what makes the output clean.

## What it will not do

**Damaged files.** An input that does not parse cleanly is refused. Guessing at a
broken structure produces a file that opens and is wrong, which is worse than an
error message.

**Encryption on the way out.** An encrypted file can be opened with `--password`,
and it comes out unencrypted. pdfsurgeon does not add encryption.

**Recompression.** Pages are copied as they are, so the output keeps the original
size apart from the pages you removed. There is no font work either.

**Signatures and form scripts.** Both are dropped. A changed document cannot
honestly carry a signature made over its old contents, and merging can rename form
fields, which would leave a script pointing at the wrong one.

**Tagged PDF and layers.** A tagged file comes out untagged, and layers come out
flat.

**Attachments.** A file attachment is only carried over when you keep every page.
Otherwise it would be attached to a document it no longer describes.

## Using it from Rust

```rust
use pdfsurgeon::{Document, PageSpec};

let doc = Document::load("report.pdf")?;
let keep = PageSpec::parse("1-3,7", doc.get_pages().len() as u32)?;
pdfsurgeon::select(&doc, keep.pages())?.save("extract.pdf")?;
```

Every command is a public function, every error is one `Error` enum, and
`lopdf::Document` is re-exported so a caller cannot end up on a different version
of it. The API docs are at <https://docs.rs/pdfsurgeon>.

The test suite runs on every change, and a nightly job feeds the parser random and
damaged files to check that it fails safely instead of crashing.

## Licence

Apache-2.0.
