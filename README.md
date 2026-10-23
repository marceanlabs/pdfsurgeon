# pdfsurgeon

[![crates.io](https://img.shields.io/crates/v/pdfsurgeon.svg)](https://crates.io/crates/pdfsurgeon)
[![downloads](https://img.shields.io/crates/d/pdfsurgeon.svg)](https://crates.io/crates/pdfsurgeon)
[![ci](https://github.com/wlkr73/pdfsurgeon/actions/workflows/ci.yml/badge.svg)](https://github.com/wlkr73/pdfsurgeon/actions/workflows/ci.yml)
[![licence](https://img.shields.io/badge/licence-Apache--2.0-blue.svg)](LICENSE)

Page level surgery for PDF files, in Rust. Merge, split, select and rotate, and
leave the rest of the document alone.

The pitch is narrow and specific: **the output does not make other PDF tools
warn**. Page removal in most libraries leaves dangling destinations behind, and
readers then report things like `Annotation destination array is too short`. This
one cleans up after itself.

## Install

```
cargo install pdfsurgeon
```

Or take a prebuilt binary from the [releases page](https://github.com/wlkr73/pdfsurgeon/releases) (Linux x86_64/aarch64, macOS x86_64/arm64, Windows x86_64).

## Speed

Measured on a 970 page, 8.6 MB document (RFC 9110 concatenated five times), best
of two runs, warm cache, same machine, against `qpdf 12.3.2` and `mutool 1.27`:

| Operation | pdfsurgeon | qpdf | mutool |
|---|---|---|---|
| Split every page into its own file | 1.07 s | 4.61 s | not supported |
| Merge two copies (1940 pages) | 0.71 s | 5.85 s | 1.25 s |
| Select 3 pages | 0.24 s | 0.22 s | 0.03 s |
| Rotate one page | 0.33 s | 2.57 s | 0.01 s |

The work here is bulk page surgery and that is where it wins. Single page edits
against a large file are dominated by parsing and writing the whole document,
which `mutool` avoids by not going through a general object model, so it stays
ahead there. `cargo run --release --example phases -- <file.pdf>` repeats the
timing.

```
$ pdfsurgeon info report.pdf
file:      report.pdf
version:   1.5
pages:     194
encrypted: false
page size: 595 x 841 pt
```

## Commands

```
pdfsurgeon info <file>
pdfsurgeon merge -o <out.pdf> <in1.pdf> <in2.pdf> ...
pdfsurgeon assemble -o <out.pdf> <file[:pages]> <file[:pages]> ...
pdfsurgeon split <file> [--out <dir>]
pdfsurgeon select <file> --pages <spec> -o <out.pdf>
pdfsurgeon rotate <file> --pages <spec> --angle <deg> -o <out.pdf>
```

Page specs accept a page, a range, or a list of either: `1`, `1-4`, `1,3-5,9`.
Angles are multiples of 90 and add to whatever rotation a page already carries,
so rotating twice by 90 leaves the page at 180 rather than resetting it.

`assemble` is the general case of `merge`: it takes pages from any number of
files, in the order you list them, and can reorder inside a file.

```
pdfsurgeon assemble -o bid.pdf cover.pdf report.pdf:2-4 appendix.pdf invoice.pdf:1
```

## As a library

The same page-graph rebuild is available as an API, for programs that would
rather add a dependency than shell out:

```rust
use pdfsurgeon::{Document, PageSpec, Piece};

let doc = Document::load("report.pdf")?;
let keep = PageSpec::parse("1-3,7", doc.get_pages().len() as u32)?;
let mut trimmed = pdfsurgeon::select(&doc, keep.pages())?;
trimmed.save("extract.pdf")?;

// pages from several files, in the order you list them
let mut bid = pdfsurgeon::assemble(
    &[cover, report, appendix],
    &[
        Piece { source: 0, pages: vec![1] },
        Piece { source: 1, pages: vec![2, 3, 4] },
        Piece::all(2, 6),
    ],
)?;
```

`merge`, `assemble`, `select`, `rotate` and `info` are all public, along with
`PageSpec`, and errors are a single `pdfsurgeon::Error` enum that implements
`std::error::Error`. `lopdf::Document` is re-exported, so callers do not have to
match the library version themselves. Docs: <https://docs.rs/pdfsurgeon>.

## Build

```
cargo build --release
./target/release/pdfsurgeon --help
```

## What is verified

Ten integration tests drive the binary and check the result, rather than
trusting the writer:

| Check | Test |
|---|---|
| Merged pages keep their order and their text | `merge_keeps_every_page_in_order` |
| Split writes one file per page | `split_writes_one_file_per_page` |
| Select keeps exactly the pages asked for | `select_keeps_only_the_requested_pages` |
| Rotation adds up across runs | `rotate_sets_and_accumulates_rotation` |
| A bad page spec exits non-zero and writes nothing | `bad_page_spec_is_rejected_without_writing` |
| An `OpenAction` into a removed page is dropped | `open_action_to_a_removed_page_is_dropped` |
| Pages from several files assemble in the order listed | `assemble_takes_pages_from_several_files_in_order` |
| One file can be reordered in place | `assemble_can_reorder_within_one_file` |
| A page number past the end is refused, nothing written | `assemble_refuses_a_page_that_does_not_exist` |
| `info` reports the true page count | `info_reports_page_count` |

`tests/edge_cases.rs` adds 40 more, against page trees written as raw bytes
because the shapes that break page surgery are the ones a library's own writer
never produces: attributes inherited from a `Pages` node, a `/Parent` cycle, a
`/Count` that lies, the same page object listed twice in `/Kids`, a zero page
document, an encrypted file, and a page asked for twice. Every output is walked
object by object to prove it holds no reference to anything that is not there.
Links get the same treatment: a named destination has to come out of a merge
pointing at a page, bookmarks from several inputs have to end up in one chain
whose ends agree, and a link that names a page the output does not contain has
to be dropped rather than left dangling.

The command line is tested the same way: flags before or after the file, a file
named `-like-a-flag` after `--`, a path with a colon in it, a path that is not
valid UTF-8, output written over the input, a write into a directory that does
not exist (nothing is written, the input is untouched, no `.part` file is left),
and four kinds of file that are not documents at all. Finally, 48 byte-mutated
copies of a real document must either work or exit 1: the process is never
allowed to crash.

Beyond the test suite, the tool was checked against real files from two
different producers, a 15 page paper and the 194 page RFC 9110, with poppler as
an independent reader:

- every output opens with no `Syntax Warning` from `pdfinfo`
- text extracted from a selection is byte identical to the same page range
  extracted from the original
- merging the 194 page RFC with the 15 page paper gives a clean 209 page file

`scripts/check_with_poppler.sh` repeats those checks on any file you point it at.

## How it works

`select` and `split` do not delete pages out of the input. They build a new
document from the objects each kept page can reach, renumbering as they go, so
stale objects, dangling destinations and dead links cannot be carried over.
That also makes it fast: splitting the 194 page RFC 9110 into 194 single page
files takes about 0.2 seconds, where deleting pages one at a time took over six
minutes.

`merge` keeps everything from every input and builds one page tree over all of
them, so links between the merged files' own pages keep working. The bookmarks
of every input are spliced into one outline, and a link that went through a
named destination is rewritten to point at the page the name resolved to, so a
merged LaTeX paper keeps both its contents list and its cross references.

Both rebuilds flatten the tree, so anything a page was inheriting from a `Pages`
node above it, its resources, media box, crop box and rotation, is written onto
the page itself on the way out. A page that leaves the old tree without those
opens blank, or at the wrong size.

## What it deliberately does not do

- **No decryption.** `info` reports an encrypted file; every command that would
  rewrite it refuses instead of writing a document whose streams it cannot read.
- **No repair.** Damaged files that lopdf cannot parse are refused, and so are
  damaged files that parse only by guesswork: a stream whose `/Length` is
  missing or does not match the bytes that were read would be copied without
  its content, so the whole command stops instead.
- **No forms, metadata or document level scripts.** `merge` builds one
  catalogue, so `/AcroForm`, `/Metadata` and JavaScript from the inputs do not
  travel. Bookmarks and internal links do: names are resolved into page
  references, and nothing is left pointing at a page that is not there.
- **Nothing is written in place.** Output goes to a temporary file beside the
  destination, is read back and checked (it must load, its catalogue must own a
  page tree, every reference in it must land on an object), and only then is it
  renamed into place. A failed write cannot leave half a document, `-o` may name
  the input file, and a file that would not open is never left behind.
- **No recompression or font work.** Streams are carried over; the output keeps
  the size of the input apart from the removed pages.
- **Links into removed pages are dropped.** A file whose links pointed at page 12
  will lose those links if page 12 is gone; keeping them would keep the warnings.
- **Bookmarks survive a merge, not an extraction.** `select`, `split` and
  `assemble` build a document from pages alone, so the outline that used to
  describe the whole file is not carried; a bookmark pointing at a page that is
  still there would be a guess about what the new document is called.
- **Text extraction is not exposed.** `info` reads structure only.

## Why another PDF tool

`qpdf` does more and has twenty years behind it. This exists because the Rust
ecosystem has heavy PDF *library* use (`lopdf` alone is past 25 million
downloads) and no command line tool with meaningful adoption: the most starred
Rust PDF CLI on GitHub has 41 stars and generates calendars. The library layer is
solved; the tool layer is not.

## Licence

Apache-2.0.
