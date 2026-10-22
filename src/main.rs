use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use lopdf::{Document, Object, ObjectId, dictionary};

type Res<T> = Result<T, Box<dyn Error>>;

const USAGE: &str = "\
pdfsurgeon - page level surgery for PDF files

USAGE:
    pdfsurgeon info <file>
    pdfsurgeon merge -o <out.pdf> <in1.pdf> <in2.pdf> ...
    pdfsurgeon assemble -o <out.pdf> <file[:pages]> <file[:pages]> ...
    pdfsurgeon split <file> [--out <dir>]
    pdfsurgeon select <file> --pages <spec> -o <out.pdf>
    pdfsurgeon rotate <file> --pages <spec> --angle <deg> -o <out.pdf>

PAGE SPECS:
    1        a single page
    1-4      a range, inclusive
    1,3-5,9  a list of either

assemble takes pages from any number of files, in the order given:
    pdfsurgeon assemble -o out.pdf report.pdf:1-3 appendix.pdf invoices.pdf:2

Angles are multiples of 90 and are added to any rotation a page already has.
";

fn main() {
    if let Err(err) = run() {
        eprintln!("pdfsurgeon: {err}");
        std::process::exit(1);
    }
}

fn run() -> Res<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(command) = args.first() else {
        print!("{USAGE}");
        return Ok(());
    };

    match command.as_str() {
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            Ok(())
        }
        "info" => {
            let file = args.get(1).ok_or("info needs a file")?;
            info(Path::new(file))
        }
        "merge" => {
            let (out, inputs) = take_out(&args[1..])?;
            let out = out.ok_or("merge needs -o <out.pdf>")?;
            let paths: Vec<PathBuf> = inputs.iter().map(PathBuf::from).collect();
            merge(&paths, &out)
        }
        "assemble" => {
            let (out, inputs) = take_out(&args[1..])?;
            let out = out.ok_or("assemble needs -o <out.pdf>")?;
            assemble(&inputs, &out)
        }
        "split" => {
            let (out, rest) = take_out(&args[1..])?;
            let file = rest.first().ok_or("split needs a file")?;
            split(Path::new(file), out)
        }
        "select" => {
            let (out, rest) = take_out(&args[1..])?;
            let out = out.ok_or("select needs -o <out.pdf>")?;
            let file = rest.first().ok_or("select needs a file")?;
            let spec = take_flag(&args[1..], "--pages").ok_or("select needs --pages")?;
            select(Path::new(file), &spec, &out)
        }
        "rotate" => {
            let (out, rest) = take_out(&args[1..])?;
            let out = out.ok_or("rotate needs -o <out.pdf>")?;
            let file = rest.first().ok_or("rotate needs a file")?;
            let spec = take_flag(&args[1..], "--pages").ok_or("rotate needs --pages")?;
            let angle: i64 = take_flag(&args[1..], "--angle")
                .ok_or("rotate needs --angle")?
                .parse()?;
            rotate(Path::new(file), &spec, angle, &out)
        }
        other => Err(format!("unknown command `{other}`\n\n{USAGE}").into()),
    }
}

/// Pull `-o <path>` or `--out <path>` out of the arguments, returning it and
/// everything else.
fn take_out(args: &[String]) -> Res<(Option<PathBuf>, Vec<String>)> {
    let mut out = None;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-o" || args[i] == "--out" {
            let value = args.get(i + 1).ok_or("-o needs a path")?;
            out = Some(PathBuf::from(value));
            i += 2;
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    Ok((out, rest))
}

/// Pull `--name <value>` out of the arguments.
fn take_flag(args: &[String], name: &str) -> Option<String> {
    let pos = args.iter().position(|a| a == name)?;
    args.get(pos + 1).cloned()
}

fn parse_pages(spec: &str, total: u32) -> Res<Vec<u32>> {
    let mut pages = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once('-') {
            Some((from, to)) => {
                let from: u32 = from.trim().parse()?;
                let to: u32 = to.trim().parse()?;
                if from == 0 || to < from {
                    return Err(format!("bad range `{part}`").into());
                }
                for n in from..=to {
                    pages.push(n);
                }
            }
            None => pages.push(part.parse()?),
        }
    }
    if pages.is_empty() {
        return Err("page spec is empty".into());
    }
    if let Some(bad) = pages.iter().find(|n| **n == 0 || **n > total) {
        return Err(format!("page {bad} is outside 1..={total}").into());
    }
    Ok(pages)
}

fn info(path: &Path) -> Res<()> {
    let doc = Document::load(path)?;
    let pages = doc.get_pages();
    println!("file:      {}", path.display());
    println!("version:   {}", doc.version);
    println!("pages:     {}", pages.len());
    println!("encrypted: {}", doc.is_encrypted());
    if let Some((_, first)) = pages.iter().next()
        && let Ok(dict) = doc.get_dictionary(*first)
        && let Ok(media) = dict.get(b"MediaBox").and_then(Object::as_array)
    {
        let nums: Vec<f32> = media.iter().filter_map(|o| o.as_float().ok()).collect();
        if nums.len() == 4 {
            println!(
                "page size: {:.0} x {:.0} pt",
                nums[2] - nums[0],
                nums[3] - nums[1]
            );
        }
    }
    Ok(())
}

fn merge(inputs: &[PathBuf], out: &Path) -> Res<()> {
    if inputs.is_empty() {
        return Err("merge needs at least one input".into());
    }

    let mut merged = Document::with_version("1.5");
    let mut page_ids: Vec<ObjectId> = Vec::new();
    let mut next_id: u32 = 1;

    for path in inputs {
        let mut doc = Document::load(path)?;
        doc.renumber_objects_with(next_id);
        next_id = doc.max_id + 1;

        page_ids.extend(doc.get_pages().into_values());

        for (id, object) in doc.objects {
            match object.type_name().unwrap_or(b"") {
                // The output gets one page tree and one catalogue of its own.
                b"Catalog" | b"Pages" => {}
                _ => {
                    merged.objects.insert(id, object);
                }
            }
        }
    }

    // Objects were inserted directly, which bypasses the id counter, so the
    // next id has to be taken from the map rather than from `max_id`.
    merged.max_id = merged.objects.keys().map(|(n, _)| *n).max().unwrap_or(0);

    let pages_id = merged.new_object_id();
    for id in &page_ids {
        if let Ok(dict) = merged.get_dictionary_mut(*id) {
            dict.set("Parent", pages_id);
        }
    }

    let kids: Vec<Object> = page_ids.iter().map(|id| Object::Reference(*id)).collect();
    let count = kids.len() as i64;
    merged.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
        }),
    );

    let catalog_id = merged.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    merged.trailer.set("Root", catalog_id);

    merged.renumber_objects();
    merged.save(out)?;
    Ok(())
}

fn split(path: &Path, out_dir: Option<PathBuf>) -> Res<()> {
    let source = Document::load(path)?;
    let total = source.get_pages().len() as u32;
    let dir = out_dir.unwrap_or_else(|| {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    });
    fs::create_dir_all(&dir)?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("page")
        .to_string();
    let by_number = source.get_pages();

    for page in 1..=total {
        let id = by_number
            .get(&page)
            .copied()
            .ok_or_else(|| format!("page {page} is missing"))?;
        let mut single = extract_pages(&source, &[id]);
        single.save(dir.join(format!("{stem}-{page:03}.pdf")))?;
    }
    Ok(())
}

/// Build a new document containing only the given pages of one source.
fn extract_pages(source: &Document, page_ids: &[ObjectId]) -> Document {
    let pages: Vec<(usize, ObjectId)> = page_ids.iter().map(|id| (0usize, *id)).collect();
    extract_from(std::slice::from_ref(source), &pages)
}

/// Takes pages from several documents, in the order listed, and builds a new
/// document holding only what those pages can reach. Nothing else travels
/// over, so no dangling reference can exist in the result.
fn extract_from(sources: &[Document], pages: &[(usize, ObjectId)]) -> Document {
    let version = sources
        .first()
        .map(|d| d.version.clone())
        .unwrap_or_else(|| "1.5".to_string());
    let mut out = Document::with_version(version);
    let mut next: u32 = 1;
    let pages_id = (next, 0);
    next += 1;

    let mut map: BTreeMap<(usize, ObjectId), ObjectId> = BTreeMap::new();
    let mut queue: Vec<(usize, ObjectId)> = Vec::new();

    for key in pages {
        if map.contains_key(key) {
            continue;
        }
        map.insert(*key, (next, 0));
        next += 1;
        queue.push(*key);
    }

    while let Some(key) = queue.pop() {
        let new = map[&key];
        let Ok(mut object) = sources[key.0].get_object(key.1).cloned() else {
            continue;
        };
        if let Object::Dictionary(dict) = &mut object
            && dict.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"Page")
        {
            // The parent is provided by the new page tree, not copied.
            dict.remove(b"Parent");
        }
        remap(&mut object, key.0, &mut map, &mut queue, &mut next);
        out.objects.insert(new, object);
    }

    let kids: Vec<Object> = pages
        .iter()
        .map(|key| Object::Reference(map[key]))
        .collect();
    out.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => pages.len() as i64,
        }),
    );

    out.max_id = next - 1;
    let catalog_id = out.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    });
    out.trailer.set("Root", catalog_id);
    out
}

/// Rewrite every reference in `object` to point into the new document,
/// queuing any object that has not been copied yet. `src` is the index of the
/// document the object came from, because ids are only unique within a source.
fn remap(
    object: &mut Object,
    src: usize,
    map: &mut BTreeMap<(usize, ObjectId), ObjectId>,
    queue: &mut Vec<(usize, ObjectId)>,
    next: &mut u32,
) {
    match object {
        Object::Reference(id) => {
            let fresh = (*next, 0);
            let assigned = *map.entry((src, *id)).or_insert_with(|| {
                *next += 1;
                queue.push((src, *id));
                fresh
            });
            *object = Object::Reference(assigned);
        }
        Object::Dictionary(dict) => {
            for (_, value) in dict.iter_mut() {
                remap(value, src, map, queue, next);
            }
        }
        Object::Array(items) => {
            for item in items.iter_mut() {
                remap(item, src, map, queue, next);
            }
        }
        Object::Stream(stream) => {
            for (_, value) in stream.dict.iter_mut() {
                remap(value, src, map, queue, next);
            }
        }
        _ => {}
    }
}

/// Assemble `file[:pages]` pieces, in the order given, into one document.
fn assemble(inputs: &[String], out: &Path) -> Res<()> {
    if inputs.is_empty() {
        return Err("assemble needs at least one input".into());
    }

    let mut sources: Vec<Document> = Vec::new();
    let mut pages: Vec<(usize, ObjectId)> = Vec::new();

    for piece in inputs {
        let (path, spec) = match piece.rsplit_once(':') {
            Some((path, spec)) if !path.is_empty() && !spec.is_empty() => (path, Some(spec)),
            _ => (piece.as_str(), None),
        };
        let doc = Document::load(path)?;
        let ids: Vec<ObjectId> = {
            let by_number = doc.get_pages();
            let total = by_number.len() as u32;
            let wanted: Vec<u32> = match spec {
                Some(spec) => parse_pages(spec, total)?,
                None => (1..=total).collect(),
            };
            let mut ids: Vec<ObjectId> = Vec::with_capacity(wanted.len());
            for n in wanted {
                let id = by_number
                    .get(&n)
                    .copied()
                    .ok_or_else(|| format!("{path}: page {n} is missing"))?;
                ids.push(id);
            }
            ids
        };
        let index = sources.len();
        pages.extend(ids.into_iter().map(|id| (index, id)));
        sources.push(doc);
    }

    let mut doc = extract_from(&sources, &pages);
    doc.save(out)?;
    Ok(())
}

fn select(path: &Path, spec: &str, out: &Path) -> Res<()> {
    let source = Document::load(path)?;
    let total = source.get_pages().len() as u32;
    let keep = parse_pages(spec, total)?;
    let by_number = source.get_pages();
    let page_ids: Vec<ObjectId> = keep
        .iter()
        .filter_map(|n| by_number.get(n).copied())
        .collect();
    let mut doc = extract_pages(&source, &page_ids);
    doc.save(out)?;
    Ok(())
}

fn rotate(path: &Path, spec: &str, angle: i64, out: &Path) -> Res<()> {
    if angle % 90 != 0 {
        return Err("angle must be a multiple of 90".into());
    }
    let mut doc = Document::load(path)?;
    let total = doc.get_pages().len() as u32;
    let pages = parse_pages(spec, total)?;
    let by_number: BTreeMap<u32, ObjectId> = doc.get_pages();

    for number in pages {
        let Some(id) = by_number.get(&number).copied() else {
            continue;
        };
        let dict = doc.get_dictionary_mut(id)?;
        let current = dict
            .get(b"Rotate")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        let updated = ((current + angle) % 360 + 360) % 360;
        dict.set("Rotate", updated);
    }

    doc.renumber_objects();
    doc.save(out)?;
    Ok(())
}
