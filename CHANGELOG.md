# Changelog

## 0.11.0

Behaviour change, and the reason this is a minor bump: **an attachment belongs to
the document it was attached to, so it no longer travels into page-level output.**

- `split` and a partial `select` used to copy every embedded file into every
  output. On a 970 page document with one 1.2 MB attachment that meant 1.3 GB of
  page files and a 1.4 MB three page selection. They are now 36 MB and 77 KB.
- Attachments still travel when every page comes along: merge, assemble of whole
  files, and a selection that keeps the whole document.
- Name tree keys are written sorted. `qpdf --check` reported "keys are not sorted"
  on the output of every command before this; it now reports no warning at all.
- `split` builds its parts on every core and hands them out as they are finished
  rather than holding all of them: 5.97 s to 1.45 s on the 970 page file, and peak
  memory from 460 MB to 235 MB.
- The binary links mimalloc. Parsing spends a fifth of its time in page faults
  from allocator churn, and this removes most of it: 30 to 40 per cent off every
  command, at a cost of about a quarter more peak memory. Build with
  `--no-default-features` for the slower, lighter version. Library users set their
  own allocator and are unaffected.
- `split_into` is new in the library: it hands each part to a sink as it is built,
  which is what the binary now uses.
- Dependencies updated.

## 0.10.1 and earlier

See the git history.
