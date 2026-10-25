#![no_main]
//! A PDF is also just bytes. This parses arbitrary bytes with the same library
//! the binary uses, then runs every operation this crate exposes on the result.

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if data.len() < 16 {
        return;
    }
    let doc = match pdfsurgeon::Document::load_mem(data) {
        Ok(d) => d,
        Err(_) => return,
    };

    let _ = pdfsurgeon::info(&doc);
    let _ = pdfsurgeon::split(&doc);
    let _ = pdfsurgeon::select(&doc, &[1, 2]);
    let _ = pdfsurgeon::select(&doc, &[0, u32::MAX, 3]);

    let mut rotated = doc.clone();
    let _ = pdfsurgeon::rotate(&mut rotated, &[1], 90);
    let _ = pdfsurgeon::rotate(&mut rotated, &[1], 45);

    if let Ok(mut merged) = pdfsurgeon::merge(vec![doc.clone(), doc.clone()]) {
        let mut out = Vec::new();
        let _ = merged.save_to(&mut out);
    }

    let piece = pdfsurgeon::Piece {
        source: 0,
        pages: vec![1],
    };
    let _ = pdfsurgeon::assemble(&[doc], &[piece]);
});
