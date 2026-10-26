#![allow(
    clippy::unwrap_used,
    reason = "a scratch example that panics on purpose"
)]

// Times the three phases of each operation separately, so optimisation can
// target the part that actually costs.
use std::time::Instant;

use pdfsurgeon::{Document, PageSpec, Piece};

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = args
        .first()
        .map(String::as_str)
        .unwrap_or("/tmp/bench/rfc.pdf");

    // load
    let t = Instant::now();
    let doc = Document::load(path).unwrap();
    let load_ms = ms(t);
    let total = doc.get_pages().len() as u32;
    println!("{path}: {total} pages, load {load_ms:.1} ms");

    // select 1..=3
    let t = Instant::now();
    let spec = PageSpec::parse("1-3", total).unwrap();
    let select_ms = ms(t);
    let t = Instant::now();
    let mut part = pdfsurgeon::select(&doc, spec.pages()).unwrap();
    let select_build = ms(t);
    let t = Instant::now();
    part.save("/tmp/bench/out_select.pdf").unwrap();
    println!(
        "select 3:  spec {select_ms:.1} ms, build {select_build:.1} ms, save {:.1} ms",
        ms(t)
    );

    // select + rotate
    let t = Instant::now();
    pdfsurgeon::rotate(&mut part, &[1], 90).unwrap();
    let rotate_build = ms(t);
    let t = Instant::now();
    part.save("/tmp/bench/out_rot.pdf").unwrap();
    println!(
        "rotate 1:  build {rotate_build:.1} ms, save {:.1} ms",
        ms(t)
    );

    // merge with itself
    let t = Instant::now();
    let other = doc.clone();
    let clone_ms = ms(t);
    let t = Instant::now();
    let mut merged = pdfsurgeon::merge(vec![doc.clone(), other]).unwrap();
    let merge_build = ms(t);
    let t = Instant::now();
    merged.save("/tmp/bench/out_merge.pdf").unwrap();
    println!(
        "merge x2:  clone {clone_ms:.1} ms, build {merge_build:.1} ms, save {:.1} ms",
        ms(t)
    );

    // assemble a mixed slice
    let t = Instant::now();
    let mut mixed = pdfsurgeon::assemble(
        &[doc.clone(), doc.clone()],
        &[Piece::new(0, (1..=total).collect()), Piece::all(1, total)],
    )
    .unwrap();
    let assemble_build = ms(t);
    let t = Instant::now();
    mixed.save("/tmp/bench/out_assemble.pdf").unwrap();
    println!(
        "assemble:  build {assemble_build:.1} ms, save {:.1} ms",
        ms(t)
    );

    // split every page, timing build and save separately across all pages
    let t = Instant::now();
    for page in 1..=total {
        let mut single = pdfsurgeon::select(&doc, &[page]).unwrap();
        single
            .save(format!("/tmp/bench/split-{page:04}.pdf"))
            .unwrap();
    }
    println!("split {total}: total {:.1} ms", ms(t));
}
