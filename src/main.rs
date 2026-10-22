use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use pdfsurgeon::{Document, PageSpec, Piece};

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
            cmd_info(Path::new(file))
        }
        "merge" => {
            let (out, inputs) = take_out(&args[1..])?;
            let out = out.ok_or("merge needs -o <out.pdf>")?;
            cmd_merge(&inputs, &out)
        }
        "assemble" => {
            let (out, inputs) = take_out(&args[1..])?;
            let out = out.ok_or("assemble needs -o <out.pdf>")?;
            cmd_assemble(&inputs, &out)
        }
        "split" => {
            let (out, rest) = take_out(&args[1..])?;
            let file = rest.first().ok_or("split needs a file")?;
            cmd_split(Path::new(file), out)
        }
        "select" => {
            let (out, rest) = take_out(&args[1..])?;
            let out = out.ok_or("select needs -o <out.pdf>")?;
            let file = rest.first().ok_or("select needs a file")?;
            let spec = take_flag(&args[1..], "--pages").ok_or("select needs --pages")?;
            cmd_select(Path::new(file), &spec, &out)
        }
        "rotate" => {
            let (out, rest) = take_out(&args[1..])?;
            let out = out.ok_or("rotate needs -o <out.pdf>")?;
            let file = rest.first().ok_or("rotate needs a file")?;
            let spec = take_flag(&args[1..], "--pages").ok_or("rotate needs --pages")?;
            let angle: i64 = take_flag(&args[1..], "--angle")
                .ok_or("rotate needs --angle")?
                .parse()?;
            cmd_rotate(Path::new(file), &spec, angle, &out)
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

fn cmd_info(path: &Path) -> Res<()> {
    let doc = Document::load(path)?;
    let info = pdfsurgeon::info(&doc);
    println!("file:      {}", path.display());
    println!("version:   {}", info.version);
    println!("pages:     {}", info.pages);
    println!("encrypted: {}", info.encrypted);
    if let Some((width, height)) = info.page_size {
        println!("page size: {width:.0} x {height:.0} pt");
    }
    Ok(())
}

fn cmd_merge(inputs: &[String], out: &Path) -> Res<()> {
    let mut docs = Vec::with_capacity(inputs.len());
    for input in inputs {
        docs.push(Document::load(input)?);
    }
    let mut merged = pdfsurgeon::merge(docs)?;
    merged.save(out)?;
    Ok(())
}

/// Split `file[:pages]` pieces into documents, then assemble them in order.
fn cmd_assemble(inputs: &[String], out: &Path) -> Res<()> {
    let mut sources: Vec<Document> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();

    for piece in inputs {
        let (path, spec) = match piece.rsplit_once(':') {
            Some((path, spec)) if !path.is_empty() && !spec.is_empty() => (path, Some(spec)),
            _ => (piece.as_str(), None),
        };
        let doc = Document::load(path)?;
        let total = doc.get_pages().len() as u32;
        let pages = match spec {
            Some(spec) => PageSpec::parse(spec, total)?.pages().to_vec(),
            None => (1..=total).collect(),
        };
        pieces.push(Piece {
            source: sources.len(),
            pages,
        });
        sources.push(doc);
    }

    let mut doc = pdfsurgeon::assemble(&sources, &pieces)?;
    doc.save(out)?;
    Ok(())
}

fn cmd_split(path: &Path, out_dir: Option<PathBuf>) -> Res<()> {
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

    for page in 1..=total {
        let mut single = pdfsurgeon::select(&source, &[page])?;
        single.save(dir.join(format!("{stem}-{page:03}.pdf")))?;
    }
    Ok(())
}

fn cmd_select(path: &Path, spec: &str, out: &Path) -> Res<()> {
    let source = Document::load(path)?;
    let total = source.get_pages().len() as u32;
    let keep = PageSpec::parse(spec, total)?;
    let mut doc = pdfsurgeon::select(&source, keep.pages())?;
    doc.save(out)?;
    Ok(())
}

fn cmd_rotate(path: &Path, spec: &str, angle: i64, out: &Path) -> Res<()> {
    let mut doc = Document::load(path)?;
    let total = doc.get_pages().len() as u32;
    let pages = PageSpec::parse(spec, total)?;
    pdfsurgeon::rotate(&mut doc, pages.pages(), angle)?;
    doc.save(out)?;
    Ok(())
}
