use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use lopdf::content::{Content, Operation};
use lopdf::{Document, Object, Stream, dictionary};

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Build a PDF whose pages each carry one line of text, so text can be traced
/// through whatever the tool does to the file.
fn make_doc(path: &Path, labels: &[&str]) {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
    });
    let resources_id = doc.add_object(dictionary! {
        "Font" => dictionary! { "F1" => font_id },
    });

    let mut kids = Vec::new();
    for label in labels {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 24.into()]),
                Operation::new("Td", vec![72.into(), 700.into()]),
                Operation::new("Tj", vec![Object::string_literal(*label)]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        });
        kids.push(Object::Reference(page_id));
    }

    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => labels.len() as i64,
        }),
    );
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    doc.trailer.set("Root", catalog_id);
    doc.save(path).unwrap();
}

fn run_cli(args: &[&str]) {
    let status = Command::new(env!("CARGO_BIN_EXE_pdfsurgeon"))
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "pdfsurgeon {args:?} failed");
}

fn pages_of(path: &Path) -> usize {
    Document::load(path).unwrap().get_pages().len()
}

fn text_of(path: &Path, page: u32) -> String {
    Document::load(path).unwrap().extract_text(&[page]).unwrap()
}

#[test]
fn info_reports_page_count() {
    let dir = scratch("info");
    let file = dir.join("three.pdf");
    make_doc(&file, &["alpha", "beta", "gamma"]);

    let out = Command::new(env!("CARGO_BIN_EXE_pdfsurgeon"))
        .args(["info", file.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains("pages:     3"), "got: {stdout}");
}

#[test]
fn merge_keeps_every_page_in_order() {
    let dir = scratch("merge");
    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    let out = dir.join("merged.pdf");
    make_doc(&a, &["a1", "a2"]);
    make_doc(&b, &["b1", "b2"]);

    run_cli(&[
        "merge",
        "-o",
        out.to_str().unwrap(),
        a.to_str().unwrap(),
        b.to_str().unwrap(),
    ]);

    assert_eq!(pages_of(&out), 4);
    assert!(text_of(&out, 1).contains("a1"));
    assert!(text_of(&out, 3).contains("b1"));
    assert!(text_of(&out, 4).contains("b2"));
}

#[test]
fn split_writes_one_file_per_page() {
    let dir = scratch("split");
    let file = dir.join("three.pdf");
    make_doc(&file, &["one", "two", "three"]);

    run_cli(&[
        "split",
        file.to_str().unwrap(),
        "--out",
        dir.to_str().unwrap(),
    ]);

    for page in 1..=3 {
        let single = dir.join(format!("three-{page:03}.pdf"));
        assert!(single.exists(), "missing {single:?}");
        assert_eq!(pages_of(&single), 1);
    }
    assert!(text_of(&dir.join("three-002.pdf"), 1).contains("two"));
}

#[test]
fn select_keeps_only_the_requested_pages() {
    let dir = scratch("select");
    let file = dir.join("four.pdf");
    let out = dir.join("picked.pdf");
    make_doc(&file, &["p1", "p2", "p3", "p4"]);

    run_cli(&[
        "select",
        file.to_str().unwrap(),
        "--pages",
        "2,4",
        "-o",
        out.to_str().unwrap(),
    ]);

    assert_eq!(pages_of(&out), 2);
    assert!(text_of(&out, 1).contains("p2"));
    assert!(text_of(&out, 2).contains("p4"));
}

#[test]
fn rotate_sets_and_accumulates_rotation() {
    let dir = scratch("rotate");
    let file = dir.join("two.pdf");
    let once = dir.join("once.pdf");
    let twice = dir.join("twice.pdf");
    make_doc(&file, &["left", "right"]);

    run_cli(&[
        "rotate",
        file.to_str().unwrap(),
        "--pages",
        "1",
        "--angle",
        "90",
        "-o",
        once.to_str().unwrap(),
    ]);
    run_cli(&[
        "rotate",
        once.to_str().unwrap(),
        "--pages",
        "1",
        "--angle",
        "90",
        "-o",
        twice.to_str().unwrap(),
    ]);

    let angle = |path: &Path| -> i64 {
        let doc = Document::load(path).unwrap();
        let id = doc.get_pages().into_values().next().unwrap();
        doc.get_dictionary(id)
            .unwrap()
            .get(b"Rotate")
            .unwrap()
            .as_i64()
            .unwrap()
    };
    assert_eq!(angle(&once), 90);
    assert_eq!(angle(&twice), 180);
    // The untouched page stays upright.
    assert_eq!(pages_of(&twice), 2);
}

/// `delete_pages` strips the page reference out of destination arrays, so a
/// catalog OpenAction pointing at a removed page turns into a broken one.
/// The output must not keep it.
#[test]
fn open_action_to_a_removed_page_is_dropped() {
    let dir = scratch("openaction");
    let file = dir.join("three.pdf");
    let out = dir.join("first.pdf");
    make_doc(&file, &["a", "b", "c"]);

    {
        let mut doc = Document::load(&file).unwrap();
        let pages: Vec<_> = doc.get_pages().into_values().collect();
        let third = pages[2];
        let action = doc.add_object(dictionary! {
            "S" => "GoTo",
            "D" => vec![Object::Reference(third), Object::Name(b"Fit".to_vec())],
        });
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        doc.get_dictionary_mut(root)
            .unwrap()
            .set("OpenAction", action);
        doc.save(&file).unwrap();
    }

    run_cli(&[
        "select",
        file.to_str().unwrap(),
        "--pages",
        "1",
        "-o",
        out.to_str().unwrap(),
    ]);

    let doc = Document::load(&out).unwrap();
    let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
    let catalog = doc.get_dictionary(root).unwrap();
    assert!(
        catalog.get(b"OpenAction").is_err(),
        "an OpenAction into a deleted page must not survive"
    );
    assert_eq!(pages_of(&out), 1);
}

#[test]
fn bad_page_spec_is_rejected_without_writing() {
    let dir = scratch("bad");
    let file = dir.join("two.pdf");
    let out = dir.join("never.pdf");
    make_doc(&file, &["x", "y"]);

    let status = Command::new(env!("CARGO_BIN_EXE_pdfsurgeon"))
        .args([
            "select",
            file.to_str().unwrap(),
            "--pages",
            "9",
            "-o",
            out.to_str().unwrap(),
        ])
        .status()
        .unwrap();

    assert!(!status.success());
    assert!(!out.exists());
}

#[test]
fn assemble_takes_pages_from_several_files_in_order() {
    let dir = scratch("assemble");
    let a = dir.join("a.pdf");
    let b = dir.join("b.pdf");
    let out = dir.join("joined.pdf");
    make_doc(&a, &["a1", "a2", "a3"]);
    make_doc(&b, &["b1", "b2"]);

    run_cli(&[
        "assemble",
        "-o",
        out.to_str().unwrap(),
        &format!("{}:1-2", a.display()),
        &format!("{}:2", b.display()),
        &format!("{}:3", a.display()),
    ]);

    assert_eq!(pages_of(&out), 4);
    assert!(text_of(&out, 1).contains("a1"));
    assert!(text_of(&out, 2).contains("a2"));
    assert!(text_of(&out, 3).contains("b2"));
    assert!(text_of(&out, 4).contains("a3"));
}

#[test]
fn assemble_can_reorder_within_one_file() {
    let dir = scratch("reorder");
    let file = dir.join("three.pdf");
    let out = dir.join("reordered.pdf");
    make_doc(&file, &["one", "two", "three"]);

    run_cli(&[
        "assemble",
        "-o",
        out.to_str().unwrap(),
        &format!("{}:3,1", file.display()),
    ]);

    assert_eq!(pages_of(&out), 2);
    assert!(text_of(&out, 1).contains("three"));
    assert!(text_of(&out, 2).contains("one"));
}

#[test]
fn assemble_refuses_a_page_that_does_not_exist() {
    let dir = scratch("assemble-bad");
    let file = dir.join("two.pdf");
    let out = dir.join("never.pdf");
    make_doc(&file, &["x", "y"]);

    let status = Command::new(env!("CARGO_BIN_EXE_pdfsurgeon"))
        .args([
            "assemble",
            "-o",
            out.to_str().unwrap(),
            &format!("{}:7", file.display()),
        ])
        .status()
        .unwrap();

    assert!(!status.success());
    assert!(!out.exists());
}
