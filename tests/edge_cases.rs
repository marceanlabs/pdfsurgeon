//! Edge cases: page trees other producers build, arguments a person will
//! actually type, and files that are not documents at all.
//!
//! Fixtures are written as raw PDF bytes rather than through lopdf, because
//! the shapes that break page surgery (attributes inherited from a Pages node,
//! `/Parent` cycles, a `/Count` that lies) are exactly the shapes lopdf's own
//! writer never produces.

#![allow(clippy::unwrap_used, reason = "tests may panic on a failed assertion")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use pdfsurgeon::lopdf::{Dictionary, Object, ObjectId, dictionary};
use pdfsurgeon::{Document, Error, PageSpec, Piece};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("edge-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a PDF from object bodies, with a correct cross reference table.
fn raw_pdf(path: &Path, objects: &[(u32, &[u8])]) {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n");
    let mut offsets = Vec::new();
    for (number, body) in objects {
        offsets.push((*number, out.len()));
        out.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = out.len();
    let size = objects.iter().map(|(n, _)| *n).max().unwrap_or(0) + 1;
    out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for number in 1..size {
        match offsets.iter().find(|(n, _)| *n == number) {
            Some((_, offset)) => {
                out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
            }
            None => out.extend_from_slice(b"0000000000 65535 f \n"),
        }
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n")
            .as_bytes(),
    );
    fs::write(path, out).unwrap();
}

fn stream(text: &str) -> Vec<u8> {
    let body = format!("BT /F1 18 Tf 72 720 Td ({text}) Tj ET\n").into_bytes();
    let mut out = format!("<< /Length {} >>\nstream\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out.extend_from_slice(b"\nendstream");
    out
}

/// Four pages in a two level tree. The middle Pages nodes hold `/MediaBox`,
/// `/Resources` and `/Rotate`, so every page inherits them.
fn nested_tree(path: &Path) {
    let s = |text: &str| stream(text);
    let (a, b, c, d) = (s("alpha"), s("beta"), s("gamma"), s("delta"));
    raw_pdf(
        path,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Count 4 /Kids [3 0 R 6 0 R] >>"),
            (
                3,
                b"<< /Type /Pages /Parent 2 0 R /Count 2 /Kids [4 0 R 5 0 R] \
                  /MediaBox [0 0 612 792] /Rotate 90 \
                  /Resources << /Font << /F1 10 0 R >> >> >>",
            ),
            (4, b"<< /Type /Page /Parent 3 0 R /Contents 7 0 R >>"),
            (5, b"<< /Type /Page /Parent 3 0 R /Contents 8 0 R >>"),
            (
                6,
                b"<< /Type /Pages /Parent 2 0 R /Count 2 /Kids [9 0 R 11 0 R] \
                  /MediaBox [0 0 612 792] \
                  /Resources << /Font << /F1 10 0 R >> >> >>",
            ),
            (7, &a),
            (8, &b),
            (9, b"<< /Type /Page /Parent 6 0 R /Contents 12 0 R >>"),
            (
                10,
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            ),
            (11, b"<< /Type /Page /Parent 6 0 R /Contents 13 0 R >>"),
            (12, &c),
            (13, &d),
        ],
    );
}

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pdfsurgeon"))
        .args(args)
        .output()
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

fn page_dict_text(doc: &Document, page: u32) -> String {
    doc.extract_text(&[page]).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// page specifications
// ---------------------------------------------------------------------------

#[test]
fn spec_refuses_a_range_past_the_end_without_building_it() {
    // `1-4000000000` against a four page file used to allocate 16 GB and abort.
    assert!(matches!(
        PageSpec::parse("1-4000000000", 4),
        Err(Error::Page {
            number: 4000000000,
            total: 4
        })
    ));
    assert!(matches!(
        PageSpec::parse("1-9", 4),
        Err(Error::Page {
            number: 9,
            total: 4
        })
    ));
    assert!(matches!(
        PageSpec::parse("5-9", 4),
        Err(Error::Page {
            number: 5,
            total: 4
        })
    ));
}

#[test]
fn spec_accepts_the_forms_people_actually_type() {
    assert_eq!(PageSpec::parse("1", 4).unwrap().pages(), &[1]);
    assert_eq!(PageSpec::parse(" 1 , 3 ", 4).unwrap().pages(), &[1, 3]);
    assert_eq!(PageSpec::parse("1,", 4).unwrap().pages(), &[1]);
    assert_eq!(PageSpec::parse(",,1,,", 4).unwrap().pages(), &[1]);
    assert_eq!(PageSpec::parse("1-1", 4).unwrap().pages(), &[1]);
    assert_eq!(PageSpec::parse("+2", 4).unwrap().pages(), &[2]);
    assert_eq!(PageSpec::parse("4-4", 4).unwrap().pages(), &[4]);
}

#[test]
fn spec_refuses_zero_reversed_and_garbage() {
    for spec in [
        "0", "0-1", "3-1", "1-2-3", "-1", "--1", "1-", "abc", "1.5", " ",
    ] {
        assert!(
            PageSpec::parse(spec, 4).is_err(),
            "`{spec}` should not parse"
        );
    }
}

#[test]
fn spec_is_one_based_and_keeps_written_order() {
    assert_eq!(PageSpec::parse("3,1-2", 3).unwrap().pages(), &[3, 1, 2]);
}

// ---------------------------------------------------------------------------
// inherited page attributes
// ---------------------------------------------------------------------------

#[test]
fn select_keeps_attributes_inherited_from_a_pages_node() {
    let dir = scratch("inherit-select");
    let file = dir.join("nested.pdf");
    nested_tree(&file);

    let source = Document::load(&file).unwrap();
    let out = pdfsurgeon::select(&source, &[1]).unwrap();
    let id = out.get_pages()[&1];
    let dict = out.get_dictionary(id).unwrap();

    assert!(dict.get(b"MediaBox").is_ok(), "media box was lost");
    assert!(dict.get(b"Resources").is_ok(), "resources were lost");
    assert!(
        dict.get(b"Parent").is_ok(),
        "the new tree should own the parent"
    );

    let saved = dir.join("one.pdf");
    let mut out = out;
    out.save(&saved).unwrap();
    let reloaded = Document::load(&saved).unwrap();
    assert!(
        page_dict_text(&reloaded, 1).contains("alpha"),
        "text should still extract, so the font resource survived"
    );
}

#[test]
fn merge_keeps_attributes_inherited_from_a_pages_node() {
    let dir = scratch("inherit-merge");
    let file = dir.join("nested.pdf");
    nested_tree(&file);

    let a = Document::load(&file).unwrap();
    let b = Document::load(&file).unwrap();
    let mut merged = pdfsurgeon::merge(vec![a, b]).unwrap();
    let saved = dir.join("merged.pdf");
    merged.save(&saved).unwrap();

    let reloaded = Document::load(&saved).unwrap();
    assert_eq!(reloaded.get_pages().len(), 8);
    for page in 1..=8 {
        let text = page_dict_text(&reloaded, page);
        assert!(!text.trim().is_empty(), "page {page} rendered nothing");
    }
}

#[test]
fn rotate_starts_from_the_rotation_a_page_inherits() {
    let dir = scratch("inherit-rotate");
    let file = dir.join("nested.pdf");
    nested_tree(&file);

    let mut source = Document::load(&file).unwrap();
    pdfsurgeon::rotate(&mut source, &[1], 90).unwrap();

    let id = source.get_pages()[&1];
    let angle = source
        .get_dictionary(id)
        .unwrap()
        .get(b"Rotate")
        .unwrap()
        .as_i64()
        .unwrap();
    assert_eq!(angle, 180, "inherited 90 plus 90 should be 180");
}

#[test]
fn info_reads_a_media_box_the_page_inherits() {
    let dir = scratch("inherit-info");
    let file = dir.join("nested.pdf");
    nested_tree(&file);

    let info = pdfsurgeon::info(&Document::load(&file).unwrap());
    assert_eq!(info.pages, 4);
    assert_eq!(info.page_size, Some((612.0, 792.0)));
}

// ---------------------------------------------------------------------------
// awkward but legal page trees
// ---------------------------------------------------------------------------

#[test]
fn a_zero_page_document_reports_zero_and_refuses_to_select() {
    let dir = scratch("zero-pages");
    let file = dir.join("empty-tree.pdf");
    raw_pdf(
        &file,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Count 0 /Kids [] >>"),
        ],
    );

    let doc = Document::load(&file).unwrap();
    assert_eq!(pdfsurgeon::info(&doc).pages, 0);
    assert!(matches!(
        pdfsurgeon::select(&doc, &[1]),
        Err(Error::Page {
            number: 1,
            total: 0
        })
    ));
}

#[test]
fn the_same_page_object_twice_in_kids_yields_two_pages() {
    let dir = scratch("dupe-kids");
    let file = dir.join("dupe.pdf");
    let s = stream("twice");
    raw_pdf(
        &file,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (
                2,
                b"<< /Type /Pages /Count 2 /Kids [3 0 R 3 0 R] /MediaBox [0 0 200 200] \
                  /Resources << /Font << /F1 5 0 R >> >> >>",
            ),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>"),
            (4, &s),
            (5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
        ],
    );

    let doc = Document::load(&file).unwrap();
    assert_eq!(doc.get_pages().len(), 2);
    let out = pdfsurgeon::select(&doc, &[1, 2]).unwrap();
    assert_eq!(out.get_pages().len(), 2);
    let id_one = out.get_pages()[&1];
    assert_eq!(id_one, out.get_pages()[&2], "one object, listed twice");
}

#[test]
fn a_page_asked_for_twice_is_copied_twice() {
    let dir = scratch("dupe-request");
    let file = dir.join("nested.pdf");
    nested_tree(&file);

    let doc = Document::load(&file).unwrap();
    let out = pdfsurgeon::select(&doc, &[1, 1]).unwrap();
    assert_eq!(out.get_pages().len(), 2);
    let mut out = out;
    let saved = dir.join("dupe.pdf");
    out.save(&saved).unwrap();
    let reloaded = Document::load(&saved).unwrap();
    assert!(page_dict_text(&reloaded, 1).contains("alpha"));
    assert!(page_dict_text(&reloaded, 2).contains("alpha"));
}

#[test]
fn a_parent_cycle_does_not_hang_or_panic() {
    let dir = scratch("cycle");
    let file = dir.join("cycle.pdf");
    let s = stream("loop");
    raw_pdf(
        &file,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (
                2,
                b"<< /Type /Pages /Count 1 /Kids [3 0 R] /Parent 3 0 R \
                  /MediaBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >> >>",
            ),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>"),
            (4, &s),
            (5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
        ],
    );

    let doc = Document::load(&file).unwrap();
    let out = pdfsurgeon::select(&doc, &[1]).unwrap();
    assert_eq!(out.get_pages().len(), 1);
}

#[test]
fn a_page_whose_parent_points_at_nothing_is_still_selectable() {
    let dir = scratch("dangling-parent");
    let file = dir.join("orphan.pdf");
    let s = stream("orphan");
    raw_pdf(
        &file,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>"),
            (
                3,
                b"<< /Type /Page /Parent 99 0 R /Contents 4 0 R \
                  /MediaBox [0 0 200 200] /Resources << /Font << /F1 5 0 R >> >> >>",
            ),
            (4, &s),
            (5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
        ],
    );

    let doc = Document::load(&file).unwrap();
    let out = pdfsurgeon::select(&doc, &[1]).unwrap();
    assert_eq!(out.get_pages().len(), 1);
}

#[test]
fn a_count_that_lies_about_the_kids_is_ignored() {
    let dir = scratch("lying-count");
    let file = dir.join("lying.pdf");
    let (s1, s2) = (stream("one"), stream("two"));
    raw_pdf(
        &file,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (
                2,
                b"<< /Type /Pages /Count 9 /Kids [3 0 R 4 0 R] /MediaBox [0 0 200 200] \
                  /Resources << /Font << /F1 6 0 R >> >> >>",
            ),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>"),
            (4, b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>"),
            (5, &s1),
            (6, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
            (7, &s2),
        ],
    );

    let doc = Document::load(&file).unwrap();
    assert_eq!(doc.get_pages().len(), 2, "kids, not count, decides");
}

#[test]
fn an_encrypted_document_is_refused_rather_than_half_read() {
    let dir = scratch("encrypted");
    let file = dir.join("locked.pdf");
    let s = stream("secret");
    raw_pdf(
        &file,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (
                2,
                b"<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 200 200] \
                  /Resources << /Font << /F1 5 0 R >> >> >>",
            ),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>"),
            (4, &s),
            (5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
            // A producer writes /Encrypt as a reference to a dictionary object.
            (6, b"<< /Filter /Standard /V 1 /R 2 /O (x) /U (y) /P -4 >>"),
        ],
    );
    // Point the trailer at it, without encrypting the streams: only the
    // refusal matters here.
    let mut bytes = fs::read(&file).unwrap();
    let trailer_at = bytes
        .windows(7)
        .rposition(|w| w == b"trailer")
        .expect("trailer");
    let patch = b"/Encrypt 6 0 R ";
    let mut patched = Vec::new();
    patched.extend_from_slice(&bytes[..trailer_at]);
    patched.extend_from_slice(b"trailer\n<< ");
    patched.extend_from_slice(patch);
    let rest = &bytes[trailer_at..];
    let after = rest
        .windows(3)
        .position(|w| w == b"<< ")
        .expect("open trailer dictionary");
    patched.extend_from_slice(&rest[after + 3..]);
    bytes = patched;
    fs::write(&file, bytes).unwrap();

    let doc = Document::load(&file).unwrap();
    assert!(doc.is_encrypted(), "fixture should look encrypted");
    assert!(matches!(
        pdfsurgeon::select(&doc, &[1]),
        Err(Error::Encrypted)
    ));
    assert!(matches!(
        pdfsurgeon::rotate(&mut Document::load(&file).unwrap(), &[1], 90),
        Err(Error::Encrypted)
    ));
    assert!(matches!(
        pdfsurgeon::merge(vec![Document::load(&file).unwrap()]),
        Err(Error::Encrypted)
    ));
    assert!(matches!(
        pdfsurgeon::assemble(&[Document::load(&file).unwrap()], &[Piece::all(0, 1)]),
        Err(Error::Encrypted)
    ));
}

#[test]
fn a_stream_with_no_length_is_refused() {
    // A damaged stream dictionary loses `/Length`, and the parser then recovers
    // by scanning for `endstream`, sometimes handing back an object with no
    // content at all. Copying that would produce a document that opens and is
    // quietly missing something, so the file has to be refused.
    let dir = scratch("bad-length");
    let file = dir.join("liar.pdf");
    let body = b"stream\nBT (x) Tj ET\nendstream";
    let mut content = b"<< >>\n".to_vec();
    content.extend_from_slice(body);
    raw_pdf(
        &file,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (
                2,
                b"<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 200 200]                   /Resources << /Font << /F1 5 0 R >> >> >>",
            ),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>"),
            (4, &content),
            (5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
        ],
    );

    let doc = Document::load(&file).unwrap();
    assert!(matches!(
        pdfsurgeon::select(&doc, &[1]),
        Err(Error::Damaged(_))
    ));
    assert!(matches!(
        pdfsurgeon::merge(vec![Document::load(&file).unwrap()]),
        Err(Error::Damaged(_))
    ));
    assert!(matches!(
        pdfsurgeon::rotate(&mut Document::load(&file).unwrap(), &[1], 90),
        Err(Error::Damaged(_))
    ));

    let out = dir.join("out.pdf");
    let status = cli(&[
        "select",
        file.to_str().unwrap(),
        "--pages",
        "1",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(!status.status.success());
    assert!(stderr(&status).contains("damaged"), "{}", stderr(&status));
    assert!(!out.exists());
}

#[test]
fn merge_does_not_trust_an_object_count_from_the_trailer() {
    // A damaged trailer can claim hundreds of millions of objects. Object ids
    // have to come from the objects that exist, or the merged file is written
    // with ids no reader will resolve.
    let dir = scratch("lying-size");
    let file = dir.join("big-size.pdf");
    let s = stream("one");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"%PDF-1.5\n%\xe2\xe3\xcf\xd3\n");
    let bodies: [(u32, &[u8]); 5] = [
        (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
        (
            2,
            b"<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 200 200]               /Resources << /Font << /F1 5 0 R >> >> >>",
        ),
        (3, b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>"),
        (4, &s),
        (5, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
    ];
    let mut offsets = Vec::new();
    for (number, body) in bodies {
        offsets.push((number, bytes.len()));
        bytes.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for (_, offset) in &offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size 441240253 /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n")
            .as_bytes(),
    );
    fs::write(&file, bytes).unwrap();

    let merged = pdfsurgeon::merge(vec![
        Document::load(&file).unwrap(),
        Document::load(&file).unwrap(),
    ])
    .unwrap();
    assert_eq!(merged.get_pages().len(), 2);
    let highest = merged.objects.keys().map(|(n, _)| *n).max().unwrap();
    assert!(highest < 100, "object ids ran away to {highest}");
}

// ---------------------------------------------------------------------------
// references in the output
// ---------------------------------------------------------------------------

/// Walk every object and require that every reference lands on something. This
/// is the invariant the whole design exists for, so it is worth checking on
/// documents that are awkward on purpose.
fn assert_no_dangling_references(doc: &Document) {
    fn walk(object: &lopdf::Object, doc: &Document, owner: &str) {
        match object {
            lopdf::Object::Reference(id) => assert!(
                doc.objects.contains_key(id),
                "object {owner} references {id:?}, which is not in the output"
            ),
            lopdf::Object::Dictionary(dict) => {
                for (_, value) in dict.iter() {
                    walk(value, doc, owner);
                }
            }
            lopdf::Object::Array(items) => {
                for value in items {
                    walk(value, doc, owner);
                }
            }
            lopdf::Object::Stream(stream) => {
                for (_, value) in stream.dict.iter() {
                    walk(value, doc, owner);
                }
            }
            _ => {}
        }
    }
    for (id, object) in &doc.objects {
        walk(object, doc, &format!("{id:?}"));
    }
}

/// Page one carries a link straight at page two, plus a link through a named
/// destination, and the catalogue owns the name tree and an outline.
fn linked_document(path: &Path) {
    let (s1, s2) = (stream("first page"), stream("second page"));
    raw_pdf(
        path,
        &[
            (
                1,
                b"<< /Type /Catalog /Pages 2 0 R /Names << /Dests 20 0 R >>                   /Outlines 30 0 R >>",
            ),
            (
                2,
                b"<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] /MediaBox [0 0 612 792]                   /Resources << /Font << /F1 10 0 R >> >> >>",
            ),
            (
                3,
                b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R /Annots [7 0 R 8 0 R] >>",
            ),
            (4, b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>"),
            (5, &s1),
            (6, &s2),
            (
                7,
                b"<< /Type /Annot /Subtype /Link /Rect [72 700 200 730]                   /Border [0 0 0] /Dest [4 0 R /Fit] >>",
            ),
            (
                8,
                b"<< /Type /Annot /Subtype /Link /Rect [72 660 220 690]                   /Border [0 0 0] /A << /S /GoTo /D (target) >> >>",
            ),
            (10, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
            (20, b"<< /Names [(target) [4 0 R /Fit]] >>"),
            (30, b"<< /Type /Outlines /First 31 0 R /Last 31 0 R /Count 1 >>"),
            (31, b"<< /Title (Chapter one) /Parent 30 0 R /Dest [3 0 R /Fit] >>"),
        ],
    );
}

/// Three pages and a two level outline: "Part one" (page 1) holds "Section A"
/// (page 2), and "Part two" sits alone on page 3.
fn outlined_document(path: &Path, looping_next: bool) {
    let (s1, s2, s3) = (stream("one"), stream("two"), stream("three"));
    let last_tail: &[u8] = if looping_next {
        b"/Parent 30 0 R /Next 33 0 R"
    } else {
        b"/Parent 30 0 R"
    };
    let object33 = format!(
        "<< /Title (Part two) /Dest [15 0 R /Fit] {} >>",
        String::from_utf8_lossy(last_tail)
    );
    raw_pdf(
        path,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R /Outlines 30 0 R >>"),
            (
                2,
                b"<< /Type /Pages /Count 3 /Kids [3 0 R 4 0 R 15 0 R] /MediaBox [0 0 612 792] >>",
            ),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>"),
            (4, b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>"),
            (15, b"<< /Type /Page /Parent 2 0 R /Contents 16 0 R >>"),
            (5, &s1),
            (6, &s2),
            (16, &s3),
            (
                30,
                b"<< /Type /Outlines /First 31 0 R /Last 33 0 R /Count 3 >>",
            ),
            (
                31,
                b"<< /Title (Part one) /Parent 30 0 R /Dest [3 0 R /Fit] /Next 33 0 R
                    /First 32 0 R /Last 32 0 R /Count 1 >>",
            ),
            (
                32,
                b"<< /Title (Section A) /Parent 31 0 R /Dest [4 0 R /Fit] >>",
            ),
            (33, object33.as_bytes()),
        ],
    );
}

fn outline_titles(doc: &Document) -> Vec<String> {
    let Ok(catalog) = doc.catalog() else {
        return Vec::new();
    };
    let Ok(root) = catalog.get(b"Outlines").and_then(|r| r.as_reference()) else {
        return Vec::new();
    };
    let Ok(first) = doc
        .get_dictionary(root)
        .and_then(|d| d.get(b"First"))
        .and_then(|f| f.as_reference())
    else {
        return Vec::new();
    };
    let mut titles = Vec::new();
    let mut cursor = Some(first);
    while let Some(id) = cursor {
        let Ok(dict) = doc.get_dictionary(id) else {
            break;
        };
        titles.push(String::from_utf8_lossy(dict.get(b"Title").unwrap().as_str().unwrap()).into());
        cursor = dict.get(b"Next").ok().and_then(|n| n.as_reference().ok());
    }
    titles
}

#[test]
fn select_brings_the_bookmarks_that_point_at_the_pages_it_kept() {
    let dir = scratch("outline-select");
    let file = dir.join("outlined.pdf");
    outlined_document(&file, false);

    let doc = Document::load(&file).unwrap();
    let picked = pdfsurgeon::select(&doc, &[1]).unwrap();
    assert_no_dangling_references(&picked);
    assert_eq!(outline_titles(&picked), vec!["Part one"]);

    // The destination has to point at the page in the new document, not at the
    // id it had in the old one.
    let catalog = picked.catalog().unwrap();
    let root = catalog.get(b"Outlines").unwrap().as_reference().unwrap();
    let item = picked
        .get_dictionary(root)
        .unwrap()
        .get(b"First")
        .unwrap()
        .as_reference()
        .unwrap();
    let dest = picked
        .get_dictionary(item)
        .unwrap()
        .get(b"Dest")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(dest[0].as_reference().unwrap(), picked.get_pages()[&1]);
}

#[test]
fn a_bookmark_whose_page_went_away_leaves_its_children_at_the_top() {
    let dir = scratch("outline-hoist");
    let file = dir.join("outlined.pdf");
    outlined_document(&file, false);

    let doc = Document::load(&file).unwrap();
    // Page 2 alone: "Part one" pointed at page 1 and goes, but "Section A"
    // pointed at page 2 and should still be reachable.
    let picked = pdfsurgeon::select(&doc, &[2]).unwrap();
    assert_no_dangling_references(&picked);
    assert_eq!(outline_titles(&picked), vec!["Section A"]);
}

#[test]
fn a_bookmark_keeps_its_children_while_its_page_is_kept() {
    let dir = scratch("outline-nested");
    let file = dir.join("outlined.pdf");
    outlined_document(&file, false);

    let doc = Document::load(&file).unwrap();
    let picked = pdfsurgeon::select(&doc, &[1, 2]).unwrap();
    assert_no_dangling_references(&picked);
    assert_eq!(outline_titles(&picked), vec!["Part one"]);

    let catalog = picked.catalog().unwrap();
    let root = catalog.get(b"Outlines").unwrap().as_reference().unwrap();
    let parent = picked
        .get_dictionary(root)
        .unwrap()
        .get(b"First")
        .unwrap()
        .as_reference()
        .unwrap();
    let dict = picked.get_dictionary(parent).unwrap();
    assert_eq!(dict.get(b"Count").unwrap().as_i64().unwrap(), 1);
    let child = dict.get(b"First").unwrap().as_reference().unwrap();
    assert_eq!(
        picked
            .get_dictionary(child)
            .unwrap()
            .get(b"Title")
            .unwrap()
            .as_str()
            .unwrap(),
        b"Section A"
    );
    assert_eq!(
        picked
            .get_dictionary(child)
            .unwrap()
            .get(b"Parent")
            .unwrap()
            .as_reference()
            .unwrap(),
        parent
    );
}

#[test]
fn split_leaves_bookmarks_out() {
    // A one page file has nothing to navigate to, and building an outline per
    // page costs a quarter of what split spends.
    let dir = scratch("outline-split");
    let file = dir.join("outlined.pdf");
    outlined_document(&file, false);
    let out = dir.join("parts");
    let status = cli(&[
        "split",
        file.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(status.status.success(), "{}", stderr(&status));

    let part = Document::load(out.join("outlined-001.pdf")).unwrap();
    assert!(part.catalog().unwrap().get(b"Outlines").is_err());
}

#[test]
fn a_looping_outline_chain_does_not_hang() {
    let dir = scratch("outline-loop");
    let file = dir.join("looping.pdf");
    outlined_document(&file, true);

    let doc = Document::load(&file).unwrap();
    let picked = pdfsurgeon::select(&doc, &[1, 2, 3]).unwrap();
    assert_no_dangling_references(&picked);
    assert_eq!(
        outline_titles(&picked),
        vec!["Part one", "Part two"],
        "each bookmark should appear once even when the chain loops"
    );
}

#[test]
fn select_output_has_no_references_to_missing_objects() {
    let dir = scratch("refs-select");
    let file = dir.join("linked.pdf");
    linked_document(&file);

    let doc = Document::load(&file).unwrap();
    for pages in [vec![1], vec![2], vec![1, 2]] {
        let out = pdfsurgeon::select(&doc, &pages).unwrap();
        assert_no_dangling_references(&out);
    }
}

#[test]
fn merge_output_has_no_references_to_missing_objects() {
    let dir = scratch("refs-merge");
    let file = dir.join("linked.pdf");
    linked_document(&file);

    let a = Document::load(&file).unwrap();
    let b = Document::load(&file).unwrap();
    let merged = pdfsurgeon::merge(vec![a, b]).unwrap();
    assert_no_dangling_references(&merged);
}

#[test]
fn assemble_output_has_no_references_to_missing_objects() {
    let dir = scratch("refs-assemble");
    let file = dir.join("linked.pdf");
    linked_document(&file);

    let a = Document::load(&file).unwrap();
    let b = Document::load(&file).unwrap();
    let out = pdfsurgeon::assemble(&[a, b], &[Piece::new(1, vec![2]), Piece::all(0, 2)]).unwrap();
    assert_no_dangling_references(&out);
}

#[test]
fn a_link_that_points_at_a_kept_page_still_points_at_it() {
    let dir = scratch("link-kept");
    let file = dir.join("linked.pdf");
    linked_document(&file);

    let doc = Document::load(&file).unwrap();
    let out = pdfsurgeon::select(&doc, &[1, 2]).unwrap();

    let page_one = out.get_pages()[&1];
    let annots = out
        .get_dictionary(page_one)
        .unwrap()
        .get(b"Annots")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    let first = annots[0].as_reference().unwrap();
    let dest = out
        .get_dictionary(first)
        .unwrap()
        .get(b"Dest")
        .unwrap()
        .as_array()
        .unwrap()
        .clone();
    let target = dest[0].as_reference().unwrap();
    assert_eq!(
        Some(&target),
        out.get_pages().get(&2),
        "the link should follow the page to its new object number"
    );
}

#[test]
fn merge_keeps_page_content_but_not_the_catalogue_name_tree() {
    // Documented limit: merge builds one page tree and one catalogue, so the
    // name tree and outline that lived in each source catalogue do not travel.
    // The links that used them become inert, and nothing dangles.
    let dir = scratch("names-merge");
    let file = dir.join("linked.pdf");
    linked_document(&file);

    let a = Document::load(&file).unwrap();
    let b = Document::load(&file).unwrap();
    let mut merged = pdfsurgeon::merge(vec![a, b]).unwrap();
    assert_eq!(merged.get_pages().len(), 4);
    let catalog = merged.catalog().unwrap();
    assert!(
        catalog.get(b"Names").is_err() || catalog.get(b"Names").unwrap().as_reference().is_err()
    );
    assert_no_dangling_references(&merged);

    let saved = dir.join("merged.pdf");
    merged.save(&saved).unwrap();
    let reloaded = Document::load(&saved).unwrap();
    assert!(page_dict_text(&reloaded, 1).contains("first page"));
    assert!(page_dict_text(&reloaded, 2).contains("second page"));
    assert!(page_dict_text(&reloaded, 3).contains("first page"));
    assert!(page_dict_text(&reloaded, 4).contains("second page"));
}

fn annots_of(doc: &Document, page: u32) -> Vec<ObjectId> {
    let id = doc.get_pages()[&page];
    doc.get_dictionary(id)
        .unwrap()
        .get(b"Annots")
        .ok()
        .and_then(|annots| annots.as_array().ok())
        .map(|annots| {
            annots
                .iter()
                .filter_map(|a| a.as_reference().ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The page a link annotation points at, following an explicit `/Dest` or a
/// `/GoTo` action's `/D`.
fn link_target(doc: &Document, annot: ObjectId) -> Option<ObjectId> {
    let dict = doc.get_dictionary(annot).ok()?;
    let destination = dict.get(b"Dest").ok().cloned().or_else(|| {
        let action = dict.get(b"A").ok()?.as_reference().ok()?;
        doc.get_dictionary(action).ok()?.get(b"D").ok().cloned()
    })?;
    let array = doc
        .get_object(match destination {
            lopdf::Object::Reference(id) => id,
            other => return other.as_array().ok().and_then(|a| a[0].as_reference().ok()),
        })
        .ok()?;
    array.as_array().ok()?.first()?.as_reference().ok()
}

#[test]
fn merge_turns_named_destinations_into_page_links() {
    let dir = scratch("named-merge");
    let file = dir.join("linked.pdf");
    linked_document(&file);

    let merged = pdfsurgeon::merge(vec![
        Document::load(&file).unwrap(),
        Document::load(&file).unwrap(),
    ])
    .unwrap();
    assert_no_dangling_references(&merged);

    // The link that used a name in the first copy now points at the second page
    // of the first copy; in the second copy, at its own second page.
    let pages = merged.get_pages();
    for (page, expected) in [(1u32, 2u32), (3, 4)] {
        let targets: Vec<_> = annots_of(&merged, page)
            .into_iter()
            .filter_map(|annot| link_target(&merged, annot))
            .collect();
        assert!(
            targets.contains(&pages[&expected]),
            "page {page} should link to page {expected}, got {targets:?}"
        );
    }
}

#[test]
fn merge_joins_the_outlines_of_every_input() {
    let dir = scratch("outline-merge");
    let file = dir.join("linked.pdf");
    linked_document(&file);

    let merged = pdfsurgeon::merge(vec![
        Document::load(&file).unwrap(),
        Document::load(&file).unwrap(),
    ])
    .unwrap();
    assert_no_dangling_references(&merged);

    let catalog = merged.catalog().unwrap();
    let root = catalog.get(b"Outlines").unwrap().as_reference().unwrap();
    let root_dict = merged.get_dictionary(root).unwrap();
    assert_eq!(root_dict.get(b"Count").unwrap().as_i64().unwrap(), 2);

    let first = root_dict.get(b"First").unwrap().as_reference().unwrap();
    let last = root_dict.get(b"Last").unwrap().as_reference().unwrap();
    assert_ne!(first, last, "two inputs should give two bookmarks");

    let first_dict = merged.get_dictionary(first).unwrap();
    let last_dict = merged.get_dictionary(last).unwrap();
    assert_eq!(
        first_dict.get(b"Parent").unwrap().as_reference().unwrap(),
        root
    );
    assert_eq!(
        last_dict.get(b"Parent").unwrap().as_reference().unwrap(),
        root
    );
    assert!(
        first_dict.get(b"Prev").is_err(),
        "first bookmark has no Prev"
    );
    assert!(last_dict.get(b"Next").is_err(), "last bookmark has no Next");
    assert_eq!(
        first_dict.get(b"Next").unwrap().as_reference().unwrap(),
        last,
        "the chains should be joined"
    );

    let pages = merged.get_pages();
    assert_eq!(
        link_target(&merged, first),
        Some(pages[&1]),
        "first bookmark should open the first page"
    );
    assert_eq!(
        link_target(&merged, last),
        Some(pages[&3]),
        "second bookmark should open the second copy's first page"
    );
}

#[test]
fn merge_adds_no_outline_when_no_input_has_one() {
    let dir = scratch("outline-none");
    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    nested_tree(&a);
    nested_tree(&b);

    let merged = pdfsurgeon::merge(vec![
        Document::load(&a).unwrap(),
        Document::load(&b).unwrap(),
    ])
    .unwrap();
    assert!(merged.catalog().unwrap().get(b"Outlines").is_err());
    assert_no_dangling_references(&merged);
}

#[test]
fn merge_keeps_the_outline_an_input_does_have() {
    let dir = scratch("outline-half");
    let with = dir.join("with.pdf");
    let without = dir.join("without.pdf");
    linked_document(&with);
    nested_tree(&without);

    let merged = pdfsurgeon::merge(vec![
        Document::load(&with).unwrap(),
        Document::load(&without).unwrap(),
    ])
    .unwrap();
    assert_no_dangling_references(&merged);
    let catalog = merged.catalog().unwrap();
    let root = catalog.get(b"Outlines").unwrap().as_reference().unwrap();
    let dict = merged.get_dictionary(root).unwrap();
    assert_eq!(dict.get(b"Count").unwrap().as_i64().unwrap(), 1);
    let first = dict.get(b"First").unwrap().as_reference().unwrap();
    assert_eq!(first, dict.get(b"Last").unwrap().as_reference().unwrap());
}

#[test]
fn select_drops_a_named_destination_that_points_at_a_removed_page() {
    let dir = scratch("named-select");
    let file = dir.join("linked.pdf");
    linked_document(&file);

    let doc = Document::load(&file).unwrap();
    let out = pdfsurgeon::select(&doc, &[1]).unwrap();
    assert_no_dangling_references(&out);

    // Page two is gone, so the link that named it cannot resolve; it is dropped
    // rather than left pointing at a name nobody defines.
    for annot in annots_of(&out, 1) {
        assert_eq!(link_target(&out, annot), None, "stale link survived");
    }
}

#[test]
fn merge_drops_the_source_cross_reference_stream() {
    // A producer that writes cross reference streams leaves an object of type
    // /XRef in the file. It describes the byte layout of that file and points
    // at its catalogue, so copying it into a merged document leaves a reference
    // to a catalogue that no longer exists.
    use lopdf::content::{Content, Operation};
    use lopdf::{Object, Stream, dictionary};

    let dir = scratch("xref-object");
    let file = dir.join("stream-xref.pdf");
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
    });
    let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
    let content = Content {
        operations: vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 24.into()]),
            Operation::new("Td", vec![72.into(), 700.into()]),
            Operation::new("Tj", vec![Object::string_literal("page")]),
            Operation::new("ET", vec![]),
        ],
    };
    let stream = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
    let page = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "Contents" => stream,
        "Resources" => resources,
        "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
    });
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page)],
            "Count" => 1i64,
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    doc.save(&file).unwrap();

    let source = Document::load(&file).unwrap();
    assert!(
        source
            .objects
            .values()
            .any(|object| matches!(object.type_name(), Ok(name) if name == b"XRef")),
        "fixture should contain a cross reference stream"
    );

    let merged = pdfsurgeon::merge(vec![
        Document::load(&file).unwrap(),
        Document::load(&file).unwrap(),
    ])
    .unwrap();
    assert!(
        !merged
            .objects
            .values()
            .any(|object| matches!(object.type_name(), Ok(name) if name == b"XRef")),
        "a stale cross reference stream should not be copied"
    );
    assert_no_dangling_references(&merged);
    assert_eq!(merged.get_pages().len(), 2);
}

#[test]
fn merge_carries_the_metadata_and_open_action_of_the_first_input() {
    let dir = scratch("catalog-extras");
    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    linked_document(&a);
    linked_document(&b);

    // Give each input its own title and a starting view, the way a producer
    // would: /Info in the trailer, /OpenAction in the catalogue.
    for (path, title) in [(&a, "First document"), (&b, "Second document")] {
        let mut doc = Document::load(path).unwrap();
        let info = doc.add_object(lopdf::dictionary! {
            "Title" => lopdf::Object::string_literal(title)
        });
        doc.trailer.set("Info", info);
        let page_one = doc.get_pages()[&1];
        doc.catalog_mut().unwrap().set(
            "OpenAction",
            vec![lopdf::Object::Reference(page_one), "Fit".into()],
        );
        doc.catalog_mut().unwrap().set("PageMode", "UseOutlines");
        doc.save(path).unwrap();
    }

    let merged = pdfsurgeon::merge(vec![
        Document::load(&a).unwrap(),
        Document::load(&b).unwrap(),
    ])
    .unwrap();
    assert_no_dangling_references(&merged);

    let info = merged.trailer.get(b"Info").unwrap().as_reference().unwrap();
    let title = merged.get_dictionary(info).unwrap().get(b"Title").unwrap();
    assert_eq!(title.as_str().unwrap(), b"First document");

    let catalog = merged.catalog().unwrap();
    assert_eq!(
        catalog.get(b"PageMode").unwrap().as_name().unwrap(),
        b"UseOutlines",
        "a merged file with bookmarks should ask for the outline panel"
    );
    let action = catalog.get(b"OpenAction").unwrap().as_array().unwrap();
    assert_eq!(
        action[0].as_reference().unwrap(),
        merged.get_pages()[&1],
        "the starting view should still be the first page"
    );
}

#[test]
fn extraction_keeps_a_self_contained_info_dictionary() {
    let dir = scratch("info-through-select");
    let file = dir.join("titled.pdf");
    linked_document(&file);
    let mut doc = Document::load(&file).unwrap();
    let info = doc.add_object(lopdf::dictionary! {
        "Title" => lopdf::Object::string_literal("Quarterly report"),
        "Author" => lopdf::Object::string_literal("A. Walker"),
    });
    doc.trailer.set("Info", info);
    doc.save(&file).unwrap();

    let picked = pdfsurgeon::select(&Document::load(&file).unwrap(), &[1]).unwrap();
    assert_no_dangling_references(&picked);
    let info = picked.trailer.get(b"Info").unwrap().as_reference().unwrap();
    let title = picked.get_dictionary(info).unwrap().get(b"Title").unwrap();
    assert_eq!(title.as_str().unwrap(), b"Quarterly report");
}

#[test]
fn an_info_dictionary_holding_a_reference_is_left_behind() {
    // Copying it would mean copying a pointer to an object the extraction has
    // no reason to keep, and the output would hold a dead reference.
    let dir = scratch("info-with-reference");
    let file = dir.join("referring.pdf");
    linked_document(&file);
    let mut doc = Document::load(&file).unwrap();
    let page = doc.get_pages()[&1];
    let info = doc.add_object(lopdf::dictionary! {
        "Title" => lopdf::Object::string_literal("Report"),
        "Trapped" => lopdf::Object::Reference(page),
    });
    doc.trailer.set("Info", info);
    doc.save(&file).unwrap();

    let picked = pdfsurgeon::select(&Document::load(&file).unwrap(), &[1]).unwrap();
    assert_no_dangling_references(&picked);
    assert!(
        picked.trailer.get(b"Info").is_err(),
        "the metadata should be dropped rather than copied with a dead pointer"
    );
}

#[test]
fn a_directory_is_reported_as_a_directory() {
    let dir = scratch("dir-input");
    let out = dir.join("out.pdf");
    let status = cli(&["info", dir.to_str().unwrap()]);
    assert!(!status.status.success());
    assert!(
        stderr(&status).contains("is a directory"),
        "{}",
        stderr(&status)
    );

    let status = cli(&[
        "select",
        dir.to_str().unwrap(),
        "--pages",
        "1",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(!status.status.success());
    assert!(!out.exists());
}

/// A two page document with a text field on page one and a radio group whose
/// two options sit on different pages. `font` is the `BaseFont` behind the
/// `/Helv` resource name, so two documents can share a name but not a font.
fn form_document(path: &Path, font: &str, with_xfa: bool, with_js: bool) {
    use lopdf::content::{Content, Operation};
    use lopdf::{Object, Stream, dictionary};

    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();

    let mut page_content = |text: &str| {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 18.into()]),
                Operation::new("Td", vec![72.into(), 700.into()]),
                Operation::new("Tj", vec![Object::string_literal(text)]),
                Operation::new("ET", vec![]),
            ],
        };
        doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()))
    };
    let content_one = page_content("page one");
    let content_two = page_content("page two");

    let resource_font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => font,
    });
    let resources = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => resource_font, "Helv" => resource_font },
    });

    // Appearance streams: a text field's and two radio states.
    let appearance = |doc: &mut Document| {
        let mut stream = Stream::new(
            dictionary! {
                "Type" => "XObject",
                "Subtype" => "Form",
                "BBox" => vec![0.into(), 0.into(), 100.into(), 20.into()],
                "Resources" => dictionary! { "Font" => dictionary! { "Helv" => resource_font } },
            },
            b"q 1 1 100 18 re S Q".to_vec(),
        );
        stream.dict.set("Length", stream.content.len() as i64);
        doc.add_object(stream)
    };
    let text_appearance = appearance(&mut doc);
    let off_appearance = appearance(&mut doc);
    let a_appearance = appearance(&mut doc);
    let b_appearance = appearance(&mut doc);

    let page_one_id = doc.new_object_id();
    let page_two_id = doc.new_object_id();

    let mut name_field = dictionary! {
        "Type" => "Annot",
        "Subtype" => "Widget",
        "FT" => "Tx",
        "T" => Object::string_literal("name"),
        "V" => Object::string_literal(""),
        "DA" => Object::string_literal("/Helv 12 Tf 0 g"),
        "Rect" => vec![72.into(), 700.into(), 250.into(), 720.into()],
        "P" => page_one_id,
        "F" => 4i64,
        "AP" => dictionary! { "N" => text_appearance },
    };
    if with_js {
        name_field.set(
            "AA",
            dictionary! {
                "K" => dictionary! {
                    "S" => "JavaScript",
                    "JS" => Object::string_literal("app.alert('hello')"),
                },
            },
        );
    }
    let name_id = doc.add_object(name_field);

    let pick_id = doc.new_object_id();
    let kid_one = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Widget",
        "FT" => "Btn",
        "Parent" => pick_id,
        "Rect" => vec![72.into(), 660.into(), 90.into(), 678.into()],
        "AS" => "Off",
        "P" => page_one_id,
        "AP" => dictionary! {
            "N" => dictionary! { "Off" => off_appearance, "a" => a_appearance },
        },
    });
    let kid_two = doc.add_object(dictionary! {
        "Type" => "Annot",
        "Subtype" => "Widget",
        "FT" => "Btn",
        "Parent" => pick_id,
        "Rect" => vec![100.into(), 660.into(), 118.into(), 678.into()],
        "AS" => "Off",
        "P" => page_two_id,
        "AP" => dictionary! {
            "N" => dictionary! { "Off" => off_appearance, "b" => b_appearance },
        },
    });
    doc.objects.insert(
        pick_id,
        Object::Dictionary(dictionary! {
            "FT" => "Btn",
            "Ff" => 49152i64,
            "T" => Object::string_literal("pick"),
            "Kids" => vec![Object::Reference(kid_one), Object::Reference(kid_two)],
        }),
    );

    doc.objects.insert(
        page_one_id,
        Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_one,
            "Resources" => resources,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Annots" => vec![Object::Reference(name_id), Object::Reference(kid_one)],
        }),
    );
    doc.objects.insert(
        page_two_id,
        Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_two,
            "Resources" => resources,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            "Annots" => vec![Object::Reference(kid_two)],
        }),
    );
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_one_id), Object::Reference(page_two_id)],
            "Count" => 2i64,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        }),
    );

    let dr = doc.add_object(dictionary! {
        "Font" => dictionary! { "Helv" => resource_font },
    });
    let mut acroform = dictionary! {
        "Fields" => vec![Object::Reference(name_id), Object::Reference(pick_id)],
        "DA" => Object::string_literal("/Helv 0 Tf 0 g"),
        "DR" => dr,
    };
    if with_xfa {
        acroform.set("XFA", Object::string_literal("<xdp:xdp/>"));
    }
    let acroform_id = doc.add_object(acroform);
    let catalog = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
        "AcroForm" => acroform_id,
    });
    doc.trailer.set("Root", catalog);
    doc.save(path).unwrap();
}

fn field_names(doc: &Document) -> Vec<String> {
    let Some(form) = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()) else {
        return Vec::new();
    };
    let form = match form {
        Object::Reference(id) => doc.get_dictionary(*id).unwrap(),
        Object::Dictionary(dict) => dict,
        _ => return Vec::new(),
    };
    form.get(b"Fields")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|field| field.as_reference().ok())
        .filter_map(|id| doc.get_dictionary(id).ok())
        .filter_map(|field| field.get(b"T").ok().and_then(|t| t.as_str().ok()))
        .map(|title| String::from_utf8_lossy(title).into_owned())
        .collect()
}

#[test]
fn merge_keeps_the_form_and_makes_the_field_names_unique() {
    let dir = scratch("form-merge");
    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    form_document(&a, "Helvetica", false, false);
    form_document(&b, "Helvetica", false, false);

    let merged = pdfsurgeon::merge(vec![
        Document::load(&a).unwrap(),
        Document::load(&b).unwrap(),
    ])
    .unwrap();
    assert_no_dangling_references(&merged);
    assert_eq!(
        field_names(&merged),
        vec!["name", "pick", "name_2", "pick_2"]
    );
}

#[test]
fn a_field_with_javascript_loses_its_actions() {
    let dir = scratch("form-js");
    let a = dir.join("a.pdf");
    form_document(&a, "Helvetica", false, true);

    let merged = pdfsurgeon::merge(vec![Document::load(&a).unwrap()]).unwrap();
    let form = merged.catalog().unwrap().get(b"AcroForm").unwrap();
    let form = match form {
        Object::Reference(id) => merged.get_dictionary(*id).unwrap(),
        Object::Dictionary(dict) => dict,
        _ => panic!("no form"),
    };
    for field in form.get(b"Fields").unwrap().as_array().unwrap() {
        let id = field.as_reference().unwrap();
        assert!(
            merged.get_dictionary(id).unwrap().get(b"AA").is_err(),
            "the script should be gone"
        );
    }
}

#[test]
fn an_xfa_form_is_left_behind_rather_than_half_carried() {
    let dir = scratch("form-xfa");
    let xfa = dir.join("xfa.pdf");
    let plain = dir.join("plain-form.pdf");
    form_document(&xfa, "Helvetica", true, false);
    form_document(&plain, "Helvetica", false, false);

    let both_xfa = pdfsurgeon::merge(vec![
        Document::load(&xfa).unwrap(),
        Document::load(&xfa).unwrap(),
    ])
    .unwrap();
    assert!(
        both_xfa.catalog().unwrap().get(b"AcroForm").is_err(),
        "an XML form cannot be merged page by page"
    );

    let mixed = pdfsurgeon::merge(vec![
        Document::load(&xfa).unwrap(),
        Document::load(&plain).unwrap(),
    ])
    .unwrap();
    assert_eq!(field_names(&mixed), vec!["name", "pick"]);
}

#[test]
fn a_font_name_shared_by_two_different_fonts_is_renamed() {
    let dir = scratch("form-fonts");
    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    form_document(&a, "Helvetica", false, false);
    form_document(&b, "Times-Roman", false, false);

    let merged = pdfsurgeon::merge(vec![
        Document::load(&a).unwrap(),
        Document::load(&b).unwrap(),
    ])
    .unwrap();
    assert_no_dangling_references(&merged);

    let form = merged.catalog().unwrap().get(b"AcroForm").unwrap();
    let form = match form {
        Object::Reference(id) => merged.get_dictionary(*id).unwrap(),
        Object::Dictionary(dict) => dict,
        _ => panic!("no form"),
    };
    let dr = form.get(b"DR").unwrap().as_reference().unwrap();
    let fonts = merged
        .get_dictionary(dr)
        .unwrap()
        .get(b"Font")
        .unwrap()
        .as_dict()
        .unwrap()
        .clone();
    let mut names: Vec<String> = fonts
        .iter()
        .map(|(name, _)| String::from_utf8_lossy(name).into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["Helv", "Helv_2"]);

    // The second document's field has to name the font it actually uses.
    let fields = form.get(b"Fields").unwrap().as_array().unwrap();
    let second = fields[2].as_reference().unwrap();
    let da = merged
        .get_dictionary(second)
        .unwrap()
        .get(b"DA")
        .unwrap()
        .as_str()
        .unwrap()
        .to_vec();
    assert!(
        da.starts_with(b"/Helv_2 "),
        "expected the renamed font, got {:?}",
        String::from_utf8_lossy(&da)
    );
}

#[test]
fn assemble_from_two_forms_gives_every_field_a_unique_name() {
    let dir = scratch("form-assemble");
    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    form_document(&a, "Helvetica", false, false);
    form_document(&b, "Helvetica", false, false);

    let assembled = pdfsurgeon::assemble(
        &[Document::load(&a).unwrap(), Document::load(&b).unwrap()],
        &[Piece::new(0, vec![1]), Piece::new(1, vec![1])],
    )
    .unwrap();
    assert_no_dangling_references(&assembled);
    assert_eq!(
        field_names(&assembled),
        vec!["name", "pick", "name_2", "pick_2"]
    );
}

#[test]
fn split_carries_the_field_of_the_page_it_wrote() {
    let dir = scratch("form-split");
    let file = dir.join("form.pdf");
    form_document(&file, "Helvetica", false, false);
    let out = dir.join("parts");
    let status = cli(&[
        "split",
        file.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert!(status.status.success(), "{}", stderr(&status));

    let first = Document::load(out.join("form-001.pdf")).unwrap();
    assert_eq!(field_names(&first), vec!["name", "pick"]);
    let second = Document::load(out.join("form-002.pdf")).unwrap();
    assert_eq!(field_names(&second), vec!["pick"]);
}

#[test]
fn select_keeps_the_fields_on_the_pages_it_kept() {
    let dir = scratch("form-select");
    let file = dir.join("form.pdf");
    form_document(&file, "Helvetica", false, false);

    let doc = Document::load(&file).unwrap();
    let first = pdfsurgeon::select(&doc, &[1]).unwrap();
    assert_no_dangling_references(&first);
    assert_eq!(field_names(&first), vec!["name", "pick"]);
    // Both pages had an option in the group; only the one on page one is left.
    let form = first.catalog().unwrap().get(b"AcroForm").unwrap();
    let form = match form {
        Object::Reference(id) => first.get_dictionary(*id).unwrap(),
        Object::Dictionary(dict) => dict,
        _ => panic!("no form"),
    };
    let pick = form.get(b"Fields").unwrap().as_array().unwrap()[1]
        .as_reference()
        .unwrap();
    let kids = first
        .get_dictionary(pick)
        .unwrap()
        .get(b"Kids")
        .unwrap()
        .as_array()
        .unwrap()
        .len();
    assert_eq!(kids, 1);

    let second = pdfsurgeon::select(&doc, &[2]).unwrap();
    assert_no_dangling_references(&second);
    assert_eq!(field_names(&second), vec!["pick"]);

    // The page that was dropped must not survive as an orphan object.
    let pages = second
        .objects
        .values()
        .filter(|object| matches!(object.type_name(), Ok(name) if name == b"Page"))
        .count();
    assert_eq!(pages, 1);
}

/// One page, an uncompressed content stream, and an XMP packet.
fn xmp_document(path: &Path) {
    let s = stream("with metadata");
    let xmp = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'><title>Report</title></x:xmpmeta>";
    let mut packet = format!(
        "<< /Type /Metadata /Subtype /XML /Length {} >>\nstream\n",
        xmp.len()
    )
    .into_bytes();
    packet.extend_from_slice(xmp);
    packet.extend_from_slice(b"\nendstream");
    raw_pdf(
        path,
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R /Metadata 10 0 R >>"),
            (
                2,
                b"<< /Type /Pages /Count 1 /Kids [3 0 R] /MediaBox [0 0 612 792] >>",
            ),
            (3, b"<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>"),
            (5, &s),
            (10, &packet),
        ],
    );
}

#[test]
fn extraction_keeps_the_xmp_packet_of_its_one_input() {
    let dir = scratch("xmp-extract");
    let file = dir.join("xmp.pdf");
    xmp_document(&file);

    let doc = Document::load(&file).unwrap();
    let picked = pdfsurgeon::select(&doc, &[1]).unwrap();
    assert_no_dangling_references(&picked);
    let metadata = picked
        .catalog()
        .unwrap()
        .get(b"Metadata")
        .unwrap()
        .as_reference()
        .unwrap();
    let stream = match picked.get_object(metadata).unwrap() {
        Object::Stream(stream) => stream,
        other => panic!("expected a stream, got {other:?}"),
    };
    assert!(
        String::from_utf8_lossy(&stream.content).contains("<title>Report</title>"),
        "the packet should be carried as it was"
    );
}

#[test]
fn a_merge_of_several_inputs_does_not_claim_one_files_xmp() {
    let dir = scratch("xmp-merge");
    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    xmp_document(&a);
    linked_document(&b);

    let merged = pdfsurgeon::merge(vec![
        Document::load(&a).unwrap(),
        Document::load(&b).unwrap(),
    ])
    .unwrap();
    assert!(
        merged.catalog().unwrap().get(b"Metadata").is_err(),
        "a packet describing one input would lie about the merged document"
    );

    // One input is still one document, so its packet can stay.
    let single = pdfsurgeon::merge(vec![Document::load(&a).unwrap()]).unwrap();
    assert!(single.catalog().unwrap().get(b"Metadata").is_ok());
}

#[test]
fn an_encrypted_file_is_opened_with_the_password_it_needs() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let locked = fixtures.join("locked-secret.pdf");
    let open = fixtures.join("locked-empty.pdf");
    let dir = scratch("encrypted-cli");

    // The owner-password-only file opens with no help.
    let out = dir.join("empty.pdf");
    let status = cli(&[
        "select",
        open.to_str().unwrap(),
        "--pages",
        "1",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(status.status.success(), "{}", stderr(&status));
    let doc = Document::load(&out).unwrap();
    assert!(!doc.was_encrypted(), "the output should not be encrypted");

    // The one with a real password says so, and takes it when given.
    let refused = dir.join("refused.pdf");
    let status = cli(&[
        "select",
        locked.to_str().unwrap(),
        "--pages",
        "1",
        "-o",
        refused.to_str().unwrap(),
    ]);
    assert!(!status.status.success());
    assert!(
        stderr(&status).contains("--password"),
        "the error should name the flag: {}",
        stderr(&status)
    );
    assert!(
        !refused.exists(),
        "nothing should be written without the password"
    );

    let opened = dir.join("opened.pdf");
    let status = cli(&[
        "select",
        locked.to_str().unwrap(),
        "--pages",
        "1",
        "--password",
        "secret",
        "-o",
        opened.to_str().unwrap(),
    ]);
    assert!(status.status.success(), "{}", stderr(&status));
    let doc = Document::load(&opened).unwrap();
    assert!(!doc.was_encrypted());
    assert_eq!(doc.get_pages().len(), 1);
    let text = doc.extract_text(&[1]).unwrap();
    assert!(text.contains("fixture one"), "got {text:?}");
}

/// A page with a text field, a signature field and an attachment, and with
/// page labels that switch from roman to decimal at page three.
fn rich_document(path: &Path) {
    let s1 = stream("one");
    let s2 = stream("two");
    let s3 = stream("three");
    let s4 = stream("four");
    let attachment = b"the attached file contents";
    let mut file_stream = format!(
        "<< /Type /EmbeddedFile /Subtype /text#2Fplain /Length {} >>\nstream\n",
        attachment.len()
    )
    .into_bytes();
    file_stream.extend_from_slice(attachment);
    file_stream.extend_from_slice(b"\nendstream");

    raw_pdf(
        path,
        &[
            (
                1,
                b"<< /Type /Catalog /Pages 2 0 R /AcroForm 20 0 R /PageLabels 25 0 R
                  /Names << /EmbeddedFiles 30 0 R >> >>",
            ),
            (
                2,
                b"<< /Type /Pages /Count 4 /Kids [3 0 R 4 0 R 5 0 R 6 0 R]
                  /MediaBox [0 0 612 792] /Resources << /Font << /F1 10 0 R >> >> >>",
            ),
            (
                3,
                b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R /Annots [11 0 R 22 0 R] >>",
            ),
            (4, b"<< /Type /Page /Parent 2 0 R /Contents 8 0 R >>"),
            (5, b"<< /Type /Page /Parent 2 0 R /Contents 15 0 R >>"),
            (6, b"<< /Type /Page /Parent 2 0 R /Contents 16 0 R >>"),
            (7, &s1),
            (8, &s2),
            (15, &s3),
            (16, &s4),
            (
                10,
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            ),
            // A text field: dictionary is both field and widget.
            (
                11,
                b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (name) /V () /F 4
                   /Rect [72 700 250 720] /P 3 0 R /DA (/Helv 12 Tf 0 g) >>",
            ),
            (20, b"<< /Fields [11 0 R 21 0 R] /DA (/Helv 0 Tf 0 g) >>"),
            // A signature field with a value: the widget sits on page one too.
            (
                21,
                b"<< /FT /Sig /T (approval) /V << /ByteRange [0 120 400 200]
                   /Contents <00ff> /Filter /Adobe.PPKLite >> /Kids [22 0 R] >>",
            ),
            (
                22,
                b"<< /Type /Annot /Subtype /Widget /Parent 21 0 R /F 4
                   /Rect [72 600 250 640] /P 3 0 R >>",
            ),
            // i, ii, then 1, 2.
            (25, b"<< /Nums [0 << /S /r >> 2 << /S /D /St 1 >>] >>"),
            (30, b"<< /Names [(readme.txt) 31 0 R] >>"),
            (
                31,
                b"<< /Type /Filespec /F (readme.txt) /UF (readme.txt)
                   /EF << /F 32 0 R >> >>",
            ),
            (32, &file_stream),
        ],
    );
}

fn form_field_ids(doc: &Document) -> Vec<ObjectId> {
    let Some(form) = doc.catalog().ok().and_then(|c| c.get(b"AcroForm").ok()) else {
        return Vec::new();
    };
    let form = match form {
        Object::Reference(id) => doc.get_dictionary(*id).unwrap(),
        Object::Dictionary(dict) => dict,
        _ => return Vec::new(),
    };
    form.get(b"Fields")
        .ok()
        .and_then(|fields| fields.as_array().ok())
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.as_reference().ok())
                .collect()
        })
        .unwrap_or_default()
}

fn resolve<'a>(object: &'a Object, doc: &'a Document) -> &'a Dictionary {
    match object {
        Object::Dictionary(dict) => dict,
        Object::Reference(id) => doc.get_dictionary(*id).unwrap(),
        other => panic!("expected a dictionary, got {other:?}"),
    }
}

fn labels_of(doc: &Document) -> Vec<String> {
    let Some(tree) = doc
        .catalog()
        .ok()
        .and_then(|catalog| catalog.get(b"PageLabels").ok())
        .and_then(|labels| labels.as_reference().ok())
    else {
        return Vec::new();
    };
    let dict = doc.get_dictionary(tree).unwrap();
    let nums = dict.get(b"Nums").unwrap().as_array().unwrap();
    let mut labels = Vec::new();
    for pair in nums.chunks(2) {
        let index = pair[0].as_i64().unwrap();
        let text = pair[1]
            .as_dict()
            .unwrap()
            .get(b"P")
            .unwrap()
            .as_str()
            .unwrap()
            .to_vec();
        labels.push(format!("{index}:{}", String::from_utf8_lossy(&text)));
    }
    labels
}

#[test]
fn a_signature_field_does_not_survive_page_surgery() {
    let dir = scratch("signature");
    let file = dir.join("signed.pdf");
    rich_document(&file);

    let doc = Document::load(&file).unwrap();
    assert_eq!(annots_of(&doc, 1).len(), 2, "fixture has two annotations");

    let picked = pdfsurgeon::select(&doc, &[1]).unwrap();
    assert_no_dangling_references(&picked);
    assert_eq!(
        form_field_ids(&picked).len(),
        1,
        "only the text field is left"
    );
    let field = picked.get_dictionary(form_field_ids(&picked)[0]).unwrap();
    assert_eq!(field.get(b"T").unwrap().as_str().unwrap(), b"name");
    assert_eq!(
        annots_of(&picked, 1).len(),
        1,
        "the signature widget has to leave the page with its field"
    );

    let merged = pdfsurgeon::merge(vec![
        Document::load(&file).unwrap(),
        Document::load(&file).unwrap(),
    ])
    .unwrap();
    assert_no_dangling_references(&merged);
    let kinds: Vec<Vec<u8>> = form_field_ids(&merged)
        .iter()
        .filter_map(|id| {
            merged
                .get_dictionary(*id)
                .ok()
                .and_then(|dict| dict.get(b"FT").ok())
                .and_then(|kind| kind.as_name().ok())
                .map(<[u8]>::to_vec)
        })
        .collect();
    assert!(
        !kinds.iter().any(|kind| kind == b"Sig"),
        "no signature fields"
    );
    assert_eq!(annots_of(&merged, 1).len(), 1);
}

#[test]
fn page_labels_are_carried_and_keep_their_text() {
    let dir = scratch("labels");
    let file = dir.join("labelled.pdf");
    rich_document(&file);

    let doc = Document::load(&file).unwrap();
    // Page two is "ii" in the input.
    let second = pdfsurgeon::select(&doc, &[2]).unwrap();
    assert_eq!(labels_of(&second), vec!["0:ii"]);

    // Page three starts the decimal run again: it is "1" there, and it is the
    // first page of the output.
    let third = pdfsurgeon::select(&doc, &[3]).unwrap();
    assert_eq!(labels_of(&third), vec!["0:1"]);

    let both = pdfsurgeon::select(&doc, &[1, 2, 3]).unwrap();
    assert_eq!(labels_of(&both), vec!["0:i", "1:ii", "2:1"]);

    let merged = pdfsurgeon::merge(vec![
        Document::load(&file).unwrap(),
        Document::load(&file).unwrap(),
    ])
    .unwrap();
    assert_eq!(
        labels_of(&merged),
        vec!["0:i", "1:ii", "2:1", "3:2", "4:i", "5:ii", "6:1", "7:2"]
    );
}

#[test]
fn attachments_travel_with_the_document() {
    let dir = scratch("attachments");
    let file = dir.join("attached.pdf");
    rich_document(&file);

    let doc = Document::load(&file).unwrap();

    // A selection of some pages is a different document, so the attachment that
    // describes the original does not come along and the output stays small.
    let part = pdfsurgeon::select(&doc, &[1]).unwrap();
    assert_no_dangling_references(&part);
    assert!(
        part.catalog().unwrap().get(b"Names").is_err(),
        "a three page selection must not carry the whole document's attachment"
    );

    // Keep every page and the document is the document, attachment and all.
    let picked = pdfsurgeon::select(&doc, &[1, 2, 3, 4]).unwrap();
    assert_no_dangling_references(&picked);
    let names = resolve(picked.catalog().unwrap().get(b"Names").unwrap(), &picked);
    let tree = names.get(b"EmbeddedFiles").unwrap().as_reference().unwrap();
    let tree = picked.get_dictionary(tree).unwrap();
    let pairs = tree.get(b"Names").unwrap().as_array().unwrap();
    assert_eq!(pairs[0].as_str().unwrap(), b"readme.txt");
    let filespec = pairs[1].as_reference().unwrap();
    let embedded = picked
        .get_dictionary(filespec)
        .unwrap()
        .get(b"EF")
        .unwrap()
        .as_dict()
        .unwrap()
        .get(b"F")
        .unwrap()
        .as_reference()
        .unwrap();
    match picked.get_object(embedded).unwrap() {
        Object::Stream(stream) => assert_eq!(stream.content, b"the attached file contents"),
        other => panic!("expected the embedded stream, got {other:?}"),
    }

    // Two documents with an attachment of the same name keep both files.
    let merged = pdfsurgeon::merge(vec![
        Document::load(&file).unwrap(),
        Document::load(&file).unwrap(),
    ])
    .unwrap();
    assert_no_dangling_references(&merged);
    let names = resolve(merged.catalog().unwrap().get(b"Names").unwrap(), &merged);
    let tree = names.get(b"EmbeddedFiles").unwrap().as_reference().unwrap();
    let tree = merged.get_dictionary(tree).unwrap();
    let pairs = tree.get(b"Names").unwrap().as_array().unwrap();
    assert_eq!(pairs.len(), 4, "two attachments, one name entry each");
    // A name tree is searched by binary search, so its keys are written sorted.
    // " (" sorts before "." in byte order, which puts the renamed copy first.
    assert_eq!(pairs[0].as_str().unwrap(), b"readme (2).txt");
    assert_eq!(pairs[2].as_str().unwrap(), b"readme.txt");
}

// ---------------------------------------------------------------------------
// command line behaviour
// ---------------------------------------------------------------------------

#[test]
fn flags_may_come_before_or_after_the_file() {
    let dir = scratch("flag-order");
    let file = dir.join("nested.pdf");
    nested_tree(&file);
    let out = dir.join("one.pdf");

    let status = cli(&[
        "select",
        "--pages",
        "1",
        "-o",
        out.to_str().unwrap(),
        file.to_str().unwrap(),
    ]);
    assert!(status.status.success(), "{}", stderr(&status));
    assert_eq!(Document::load(&out).unwrap().get_pages().len(), 1);
}

#[test]
fn a_file_named_like_a_flag_is_reachable_after_a_double_dash() {
    let dir = scratch("dash-file");
    let file = dir.join("-dash.pdf");
    nested_tree(&file);
    let out = dir.join("out.pdf");

    let status = cli(&[
        "select",
        "--pages",
        "1",
        "-o",
        out.to_str().unwrap(),
        "--",
        file.to_str().unwrap(),
    ]);
    assert!(status.status.success(), "{}", stderr(&status));
    assert_eq!(Document::load(&out).unwrap().get_pages().len(), 1);
}

#[test]
fn a_second_file_is_an_error_not_something_ignored() {
    let dir = scratch("extra-arg");
    let file = dir.join("nested.pdf");
    nested_tree(&file);
    let out = dir.join("out.pdf");

    let status = cli(&[
        "select",
        file.to_str().unwrap(),
        file.to_str().unwrap(),
        "--pages",
        "1",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(!status.status.success());
    assert!(
        stderr(&status).contains("unexpected argument"),
        "{}",
        stderr(&status)
    );
    assert!(!out.exists());
}

#[test]
fn a_path_with_a_colon_is_a_path_not_a_page_spec() {
    let dir = scratch("colon-path");
    let odd = dir.join("odd:name.pdf");
    nested_tree(&odd);
    let out = dir.join("joined.pdf");

    let status = cli(&[
        "assemble",
        "-o",
        out.to_str().unwrap(),
        odd.to_str().unwrap(),
    ]);
    assert!(status.status.success(), "{}", stderr(&status));
    assert_eq!(Document::load(&out).unwrap().get_pages().len(), 4);
}

#[test]
fn select_in_place_leaves_a_readable_file() {
    let dir = scratch("in-place");
    let file = dir.join("nested.pdf");
    nested_tree(&file);

    let status = cli(&[
        "select",
        file.to_str().unwrap(),
        "--pages",
        "2",
        "-o",
        file.to_str().unwrap(),
    ]);
    assert!(status.status.success(), "{}", stderr(&status));
    let doc = Document::load(&file).unwrap();
    assert_eq!(doc.get_pages().len(), 1);
    assert!(page_dict_text(&doc, 1).contains("beta"));
}

#[test]
fn a_failed_write_leaves_the_input_alone_and_no_debris() {
    let dir = scratch("failed-write");
    let file = dir.join("nested.pdf");
    nested_tree(&file);
    let before = fs::read(&file).unwrap();
    let out = dir.join("missing").join("out.pdf");

    let status = cli(&[
        "select",
        file.to_str().unwrap(),
        "--pages",
        "1",
        "-o",
        out.to_str().unwrap(),
    ]);
    assert!(!status.status.success());
    assert_eq!(fs::read(&file).unwrap(), before, "input was modified");
    assert!(!out.exists());
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".part"))
        .collect();
    assert!(leftovers.is_empty(), "temporary files left behind");
}

#[test]
fn out_of_range_pages_write_nothing() {
    let dir = scratch("out-of-range");
    let file = dir.join("nested.pdf");
    nested_tree(&file);
    let out = dir.join("never.pdf");

    for spec in ["0", "5", "3-9", "1-4000000000"] {
        let status = cli(&[
            "select",
            file.to_str().unwrap(),
            "--pages",
            spec,
            "-o",
            out.to_str().unwrap(),
        ]);
        assert!(!status.status.success(), "`{spec}` should fail");
        assert!(!out.exists(), "`{spec}` wrote a file anyway");
    }
}

#[test]
fn split_creates_a_nested_output_directory_and_refuses_a_file() {
    let dir = scratch("split-out");
    let file = dir.join("nested.pdf");
    nested_tree(&file);

    let nested = dir.join("a").join("b");
    let status = cli(&[
        "split",
        file.to_str().unwrap(),
        "--out",
        nested.to_str().unwrap(),
    ]);
    assert!(status.status.success(), "{}", stderr(&status));
    assert!(nested.join("nested-004.pdf").exists());

    let blocker = dir.join("plain-file");
    fs::write(&blocker, b"not a directory").unwrap();
    let status = cli(&[
        "split",
        file.to_str().unwrap(),
        "--out",
        blocker.to_str().unwrap(),
    ]);
    assert!(!status.status.success());
}

#[test]
fn anything_that_is_not_a_document_fails_without_writing() {
    let dir = scratch("not-a-document");
    let out = dir.join("out.pdf");

    let cases: [(&str, Vec<u8>); 4] = [
        ("empty.pdf", Vec::new()),
        ("header-only.pdf", b"%PDF-1.5\n".to_vec()),
        ("png.pdf", b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR".to_vec()),
        (
            "random.pdf",
            (0..2048u32).map(|i| (i * 37 % 251) as u8).collect(),
        ),
    ];

    for (name, bytes) in cases {
        let file = dir.join(name);
        fs::write(&file, bytes).unwrap();
        let status = cli(&["info", file.to_str().unwrap()]);
        assert!(!status.status.success(), "`{name}` should fail to load");
        assert!(!stderr(&status).is_empty(), "`{name}` failed silently");

        let status = cli(&[
            "select",
            file.to_str().unwrap(),
            "--pages",
            "1",
            "-o",
            out.to_str().unwrap(),
        ]);
        assert!(!status.status.success(), "`{name}` should not select");
        assert!(!out.exists(), "`{name}` produced output");
    }
}

#[test]
fn a_truncated_document_is_refused() {
    let dir = scratch("truncated");
    let file = dir.join("nested.pdf");
    nested_tree(&file);
    let bytes = fs::read(&file).unwrap();

    // A file cut mid structure has to be refused. A file missing only its
    // closing bytes may still be readable, which is fine, as long as the
    // attempt never crashes.
    for cut in [40, 200, bytes.len() / 2] {
        let short = dir.join(format!("cut-{cut}.pdf"));
        fs::write(&short, &bytes[..cut]).unwrap();
        let status = cli(&["info", short.to_str().unwrap()]);
        assert!(
            !status.status.success(),
            "a file cut at {cut} bytes should be refused"
        );
    }
    let short = dir.join("cut-tail.pdf");
    fs::write(&short, &bytes[..bytes.len() - 20]).unwrap();
    let status = cli(&["info", short.to_str().unwrap()]);
    assert!(matches!(status.status.code(), Some(0) | Some(1)));
}

#[cfg(unix)]
#[test]
fn a_path_that_is_not_valid_utf8_still_opens() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = scratch("non-utf8");
    let mut name = OsString::from("bad");
    name.push(OsString::from_vec(vec![0xff]));
    name.push(".pdf");
    let file = dir.join(&name);
    nested_tree(&file);

    let status = Command::new(env!("CARGO_BIN_EXE_pdfsurgeon"))
        .arg("info")
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(String::from_utf8_lossy(&status.stdout).contains("pages:     4"));
}

#[test]
fn the_version_is_reported() {
    let out = cli(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("pdfsurgeon {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn merging_many_inputs_keeps_every_page_and_every_bookmark() {
    let dir = scratch("many-inputs");
    let file = dir.join("linked.pdf");
    linked_document(&file);

    let sources: Vec<Document> = (0..30).map(|_| Document::load(&file).unwrap()).collect();
    let merged = pdfsurgeon::merge(sources).unwrap();
    assert_eq!(merged.get_pages().len(), 60);
    assert_no_dangling_references(&merged);

    let catalog = merged.catalog().unwrap();
    let root = catalog.get(b"Outlines").unwrap().as_reference().unwrap();
    assert_eq!(
        merged
            .get_dictionary(root)
            .unwrap()
            .get(b"Count")
            .unwrap()
            .as_i64()
            .unwrap(),
        30
    );

    // Walk the joined chain and count it: one bookmark per input.
    let mut current = merged
        .get_dictionary(root)
        .unwrap()
        .get(b"First")
        .unwrap()
        .as_reference()
        .unwrap();
    let mut seen = 0;
    loop {
        let dict = merged.get_dictionary(current).unwrap();
        assert_eq!(
            dict.get(b"Parent").unwrap().as_reference().unwrap(),
            root,
            "every top level bookmark must belong to the one root"
        );
        seen += 1;
        match dict
            .get(b"Next")
            .ok()
            .and_then(|next| next.as_reference().ok())
        {
            Some(next) => current = next,
            None => break,
        }
    }
    assert_eq!(seen, 30);
}

#[test]
fn the_help_text_and_a_bare_invocation_succeed() {
    let bare = cli(&[]);
    assert!(bare.status.success());
    assert!(String::from_utf8_lossy(&bare.stdout).contains("USAGE"));
    assert!(cli(&["--help"]).status.success());
    assert!(!cli(&["nonsense"]).status.success());
}

// ---------------------------------------------------------------------------
// mutated files must never crash the process
// ---------------------------------------------------------------------------

#[test]
fn mutated_documents_fail_cleanly_and_never_crash() {
    // Both shapes: a plain nested tree, and one carrying links, a name tree and
    // an outline, so the destination and bookmark code is fuzzed too.
    type Fixture = (&'static str, fn(&Path));
    let fixtures: [Fixture; 2] = [("nested", nested_tree), ("linked", linked_document)];
    let dir = scratch("fuzz");
    let mut state: u64 = 0x5eed;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for (name, build) in fixtures {
        let base = dir.join(format!("{name}-base.pdf"));
        build(&base);
        let original = fs::read(&base).unwrap();
        for case in 0..48 {
            let mut bytes = original.clone();
            let flips = 1 + (next() % 8) as usize;
            for _ in 0..flips {
                let at = (next() as usize) % bytes.len();
                bytes[at] = (next() % 256) as u8;
            }
            let file = dir.join(format!("{name}-mutated-{case}.pdf"));
            fs::write(&file, &bytes).unwrap();

            let out = dir.join(format!("{name}-mutated-{case}-out.pdf"));
            for args in [
                vec!["info", file.to_str().unwrap()],
                vec![
                    "select",
                    file.to_str().unwrap(),
                    "--pages",
                    "1",
                    "-o",
                    out.to_str().unwrap(),
                ],
                vec!["merge", "-o", out.to_str().unwrap(), file.to_str().unwrap()],
            ] {
                let output = cli(&args);
                let code = output.status.code();
                assert!(
                    code == Some(0) || code == Some(1),
                    "case {case} {args:?} ended with {code:?}: {}",
                    stderr(&output)
                );
                if code == Some(0) && args[0] != "info" {
                    // Anything it claims to have written has to be readable.
                    Document::load(&out).unwrap_or_else(|err| {
                        panic!("case {name} {case} {args:?} wrote an unreadable file: {err}")
                    });
                }
            }
        }
    }
}
