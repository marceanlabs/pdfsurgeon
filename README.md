# pdfsurgeon

Page level surgery for PDF files, in Rust. Merge, split, select and rotate, and
leave the rest of the document alone.

The pitch is narrow and specific: **the output does not make other PDF tools
warn**. Page removal in most libraries leaves dangling destinations behind, and
readers then report things like `Annotation destination array is too short`. This
one cleans up after itself.

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
pdfsurgeon split <file> [--out <dir>]
pdfsurgeon select <file> --pages <spec> -o <out.pdf>
pdfsurgeon rotate <file> --pages <spec> --angle <deg> -o <out.pdf>
```

Page specs accept a page, a range, or a list of either: `1`, `1-4`, `1,3-5,9`.
Angles are multiples of 90 and add to whatever rotation a page already carries,
so rotating twice by 90 leaves the page at 180 rather than resetting it.

## Build

```
cargo build --release
./target/release/pdfsurgeon --help
```

## What is verified

Seven integration tests build throwaway documents and check the result, rather
than trusting the writer:

| Check | Test |
|---|---|
| Merged pages keep their order and their text | `merge_keeps_every_page_in_order` |
| Split writes one file per page | `split_writes_one_file_per_page` |
| Select keeps exactly the pages asked for | `select_keeps_only_the_requested_pages` |
| Rotation adds up across runs | `rotate_sets_and_accumulates_rotation` |
| A bad page spec exits non-zero and writes nothing | `bad_page_spec_is_rejected_without_writing` |
| An `OpenAction` into a removed page is dropped | `open_action_to_a_removed_page_is_dropped` |
| `info` reports the true page count | `info_reports_page_count` |

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
them, so links between the merged files' own pages keep working.

## What it deliberately does not do

- **No decryption.** Encrypted files are reported, not opened.
- **No repair.** Damaged files that lopdf cannot parse are refused.
- **No recompression or font work.** Streams are carried over; the output keeps
  the size of the input apart from the removed pages.
- **Links into removed pages are dropped.** A file whose links pointed at page 12
  will lose those links if page 12 is gone; keeping them would keep the warnings.
- **No bookmarks or forms support** beyond what survives untouched.
- **Text extraction is not exposed.** `info` reads structure only.

## Why another PDF tool

`qpdf` does more and has twenty years behind it. This exists because the Rust
ecosystem has heavy PDF *library* use (`lopdf` alone is past 25 million
downloads) and no command line tool with meaningful adoption: the most starred
Rust PDF CLI on GitHub has 41 stars and generates calendars. The library layer is
solved; the tool layer is not.

## Licence

Apache-2.0.
