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
| Split every page into its own file | 0.89 s | 4.81 s | not supported |
| Merge two copies (1,940 pages) | 0.98 s | 6.06 s | 1.32 s |
| Select 3 pages | 0.27 s | 2.27 s | 0.69 s |
| Rotate every page (`mutool` has no per-page rotate) | 0.37 s | 2.55 s | 0.01 s |

The work here is bulk page surgery and that is where it wins. A single page edit
against a large file is dominated by parsing and writing the whole document,
which `mutool` avoids by not going through a general object model, so it stays
ahead on that one. `cargo run --release --example phases -- <file.pdf>` repeats
the timing.

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
pdfsurgeon --version
pdfsurgeon info <file>
pdfsurgeon merge -o <out.pdf> <in1.pdf> <in2.pdf> ...
pdfsurgeon assemble -o <out.pdf> <file[:pages]> <file[:pages]> ...
pdfsurgeon split <file> [--out <dir>]
pdfsurgeon select <file> --pages <spec> -o <out.pdf>
pdfsurgeon rotate <file> --pages <spec> --angle <deg> -o <out.pdf>
```

`--password <pw>` opens an encrypted input and works on every command that reads
a file.

Split names its files `<name>-<page>.pdf` with at least three digits, and more
when the page count needs them, so they sort beside the original.

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

`tests/edge_cases.rs` adds 66 more, against page trees written as raw bytes
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
four kinds of file that are not documents at all, and a directory (which is
named as one instead of surfacing as "IO error"). Finally, 48 byte-mutated
copies of a real document must either work or exit 1: the process is never
allowed to crash.

Beyond the test suite, the tool was checked against real files from two
different producers, a 15 page paper and the 194 page RFC 9110, with poppler as
an independent reader:

- every output opens with no `Syntax Warning` from `pdfinfo`
- text extracted from a selection is byte identical to the same page range
  extracted from the original
- merging the 194 page RFC with the 15 page paper gives a clean 209 page file

Four page specs (`1,3,5`, a range, the last two pages, a single page) are also
extracted with `qpdf` as a second opinion, and the text has to match what `qpdf`
puts in its own copy of the same pages.

Forms are checked against `pypdf`: a merge of two forms has to expose every field
(the clash renamed), each one has to accept a value that reads back, a selection
has to keep only the fields on the pages it kept, and every widget has to still
be attached to its page. `scripts/check_forms.py` builds the forms with
`reportlab` and exits non-zero when any of that stops being true; CI runs it.

`scripts/check_with_poppler.sh` repeats those checks on any file you point it at.

## How it works

`select` and `split` do not delete pages out of the input. They build a new
document from the objects each kept page can reach, renumbering as they go, so
stale objects, dangling destinations and dead links cannot be carried over.
That also makes it fast: splitting the 194 page RFC 9110 into 194 single page
files takes about a quarter of a second, where deleting pages one at a time
took over six minutes.

`merge` keeps everything from every input and builds one page tree over all of
them, so links between the merged files' own pages keep working. The bookmarks
of every input are spliced into one outline, and a link that went through a
named destination is rewritten to point at the page the name resolved to, so a
merged LaTeX paper keeps both its contents list and its cross references. The
first input's `/Info` dictionary, its starting view and its request to show the
outline panel come across with it.

Both rebuilds flatten the tree, so anything a page was inheriting from a `Pages`
node above it, its resources, media box, crop box and rotation, is written onto
the page itself on the way out. A page that leaves the old tree without those
opens blank, or at the wrong size.

Bookmarks are rebuilt rather than copied: the outline of the input is walked,
every destination is resolved to the page it names, and only the items whose page
survived are written into a fresh tree in the new document's numbering. Selecting
pages 1-40 of RFC 9110 gives a file whose outline still has its three levels and
whose entries all land inside those 40 pages.

## What it deliberately does not do

- **Encryption is read, never written.** An encrypted input is opened with
  `--password`; without one the empty password is tried, which is all a document
  with an owner password needs. The output is always unencrypted, and the
  permissions it carried (no printing, no copying) go with the encryption. A
  file whose password is not known is refused with the flag named, because the
  alternative is a document that parses and then yields an empty page tree.
- **No repair.** Damaged files that lopdf cannot parse are refused, and so are
  damaged files that parse only by guesswork: a stream whose `/Length` is
  missing or does not match the bytes that were read would be copied without
  its content, so the whole command stops instead.
- **Metadata travels only when it is self-contained.** The first input's
  `/Info` dictionary (title, author, subject, keywords, dates) is carried through
  a merge and through an extraction, and its `/OpenAction` and `/PageMode` are
  kept when the page they name survives. An `/Info` that holds a reference is
  left behind rather than copied with a pointer into a document that no longer
  exists. The XMP `/Metadata` packet travels the same way when there is one
  input, including a merge of one; a merge of several drops it, because a packet
  describing one file would be a false statement about the result. JavaScript
  does not travel at all.
- **Forms travel as forms, not as pictures.** `merge`, `assemble`, `select` and
  `split` all rebuild `/AcroForm` around the widgets that are actually in the
  output. Fields keep their type, flags, values and appearance streams; a field
  whose page was removed is not in the form, and a widget is never left without
  its field. Two inputs that both have a field called `name` come out as `name`
  and `name_2`, because a duplicate makes every reference to it ambiguous. Fonts
  in `/DR` are shared when the name means the same face and renamed (with the
  appearance strings rewritten) when it does not.
- **No scripts, no XFA.** Field actions (`/AA`) are dropped: a calculation or
  validation script names fields by full name, and a merge may have changed it.
  An XFA form is XML the viewer renders instead of the pages, so it is left
  behind rather than half carried.
- **Signatures are dropped, not broken.** A signature covers the bytes of the
  file it was made in, and page surgery changes them. The field and its widget
  are removed rather than carried as a signature that fails the moment a reader
  checks it. Everything else about the page is untouched.
- **Page labels travel.** A file that numbers its front matter i, ii, iii and the
  rest 1, 2, 3 keeps that numbering through a selection, a split and a merge;
  the tree is written out one page at a time so the text is reproduced exactly
  rather than re-derived.
- **Attachments travel.** Embedded files are not page-bound, so they come with
  the document. A merge that brings two files of the same name keeps both,
  renaming the second `note (2).txt` rather than losing one.
- **Accessibility tagging and layers do not travel.** A structure tree
  (`/StructTreeRoot`) describes the document it was built for, and layer
  membership lives in the content streams; rebuilding either is a different
  program from this one. A tagged PDF comes out untagged and its layers come out
  flat.
- **Nothing is written in place.** The document is checked before it is written
  (its catalogue must own a page tree, every reference in it must land on an
  object, no stream may be missing its length), written to a temporary file
  beside the destination, and renamed into place. A failed write cannot leave
  half a document, `-o` may name the input file, and a document that would not
  open is never left behind.
- **No recompression and no font work.** Streams are carried over as they are, so
  the output keeps the size of the input apart from the removed pages. Rewriting
  them for size is a different job, and one that would put content at risk for a
  number that does not matter to this tool.
- **Links into removed pages are dropped.** A file whose links pointed at page 12
  will lose those links if page 12 is gone; keeping them would keep the warnings.
- **Bookmarks are pruned, not invented.** A merge splices every input's outline
  into one tree. `select` and `assemble` keep the bookmarks whose page came
  along, drop the rest, and lift the children of a dropped section up a level so
  a jump that still works is not lost with its heading. `split` writes one page
  files without an outline: there is nowhere in a single page to travel to.
- **Text extraction is not exposed.** `info` reads structure only.

## Why another PDF tool

`qpdf` does more and has twenty years behind it. This exists because the Rust
ecosystem has heavy PDF *library* use (`lopdf` alone is past 25 million
downloads) and no command line tool with meaningful adoption: the most starred
Rust PDF CLI on GitHub has 41 stars and generates calendars. The library layer is
solved; the tool layer is not.

## Licence

Apache-2.0.
