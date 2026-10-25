# pdfsurgeon

[![crates.io](https://img.shields.io/crates/v/pdfsurgeon.svg)](https://crates.io/crates/pdfsurgeon)
[![ci](https://github.com/marceanlabs/pdfsurgeon/actions/workflows/ci.yml/badge.svg)](https://github.com/marceanlabs/pdfsurgeon/actions/workflows/ci.yml)

Take pages out of a PDF, put PDFs together, or split one into many, without
breaking it.

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

## Table of Contents

- [Install](#install)
- [Usage](#usage)
- [What it will not do](#what-it-will-not-do)
- [Performance](#performance)
- [Using it from Rust](#using-it-from-rust)
- [Contributing](#contributing)
- [Licence](#licence)

## Install

```console
$ cargo install pdfsurgeon
```

Building from source needs Rust 1.85 or newer. Prebuilt binaries are on the
[releases page](https://github.com/marceanlabs/pdfsurgeon/releases):

| Platform | Target |
|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux arm64 | `aarch64-unknown-linux-gnu` |
| macOS, Apple silicon | `aarch64-apple-darwin` |
| macOS, Intel | `x86_64-apple-darwin` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

## Usage

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

### Exit codes

| Code | Meaning |
|---|---|
| 0 | it worked |
| 1 | it did not, including a command line it could not read |

Bad arguments and failed work share one code here. A script should check for zero
rather than try to tell the two apart.

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

## Performance

On a 970 page, 13 MB document, splitting every page takes 1.02 s against 3.71 s for
qpdf 12.3.2, and merging 1,940 pages takes 0.75 s against 5.36 s. mutool is quicker
at selecting three pages, and there is a real reason for that.

The full table, the method and the reason are in [BENCHMARKS.md](BENCHMARKS.md).

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

## Contributing

Questions and bug reports go in the [issues](https://github.com/marceanlabs/pdfsurgeon/issues).
Please open one before writing a patch, since the scope here is narrow on purpose
and a feature that falls outside it will be turned down whatever its quality.

Security problems should go through [private reporting](https://github.com/marceanlabs/pdfsurgeon/security/advisories/new)
rather than a public issue.

## Licence

Apache-2.0.
