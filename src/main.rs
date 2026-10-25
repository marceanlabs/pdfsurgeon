use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use pdfsurgeon::{Document, PageSpec, Piece};

type Res<T> = Result<T, Box<dyn Error>>;

const USAGE: &str = "\
pdfsurgeon - page level surgery for PDF files

USAGE:
    pdfsurgeon --version
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

Flags may come before or after the files. `--` ends flag parsing, so a file
whose name starts with a dash can still be named.

An encrypted input is opened with `--password`: without one the empty password
is tried, which is all a file with only an owner password needs. The output is
never encrypted.
";

// The parser allocates a dictionary and a buffer for every object, and glibc's
// malloc turns that into a fifth of the run time spent in page faults. Measured
// on a 970 page document: 307 ms to parse with glibc, 231 ms with mimalloc.
#[cfg(feature = "mimalloc-alloc")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    if let Err(err) = run() {
        eprintln!("pdfsurgeon: {err}");
        std::process::exit(1);
    }
}

fn run() -> Res<()> {
    // OsString, not String: a path that is not valid UTF-8 is still a path,
    // and `env::args()` would panic on it.
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let Some(command) = args.first() else {
        print!("{USAGE}");
        return Ok(());
    };

    match command.to_str().unwrap_or("") {
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            Ok(())
        }
        "-V" | "--version" | "version" => {
            println!("pdfsurgeon {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "info" => {
            let parsed = parse_args(&args[1..])?;
            one_file(&parsed)?;
            let file = parsed.positional.first().ok_or("info needs a file")?;
            cmd_info(Path::new(file), &parsed)
        }
        "merge" => {
            let args = parse_args(&args[1..])?;
            let out = args.out.clone().ok_or("merge needs -o <out.pdf>")?;
            cmd_merge(&args.positional, &out, &args)
        }
        "assemble" => {
            let args = parse_args(&args[1..])?;
            let out = args.out.clone().ok_or("assemble needs -o <out.pdf>")?;
            cmd_assemble(&args.positional, &out, &args)
        }
        "split" => {
            let args = parse_args(&args[1..])?;
            one_file(&args)?;
            let file = args.positional.first().ok_or("split needs a file")?;
            cmd_split(Path::new(file), args.out.clone(), &args)
        }
        "select" => {
            let args = parse_args(&args[1..])?;
            one_file(&args)?;
            let out = args.out.clone().ok_or("select needs -o <out.pdf>")?;
            let file = args.positional.first().ok_or("select needs a file")?;
            let spec = args.pages.clone().ok_or("select needs --pages")?;
            cmd_select(Path::new(file), &spec, &out, &args)
        }
        "rotate" => {
            let args = parse_args(&args[1..])?;
            one_file(&args)?;
            let out = args.out.clone().ok_or("rotate needs -o <out.pdf>")?;
            let file = args.positional.first().ok_or("rotate needs a file")?;
            let spec = args.pages.clone().ok_or("rotate needs --pages")?;
            let angle: i64 = args
                .angle
                .clone()
                .ok_or("rotate needs --angle")?
                .parse()
                .map_err(|_| "angle must be a whole number of degrees")?;
            cmd_rotate(Path::new(file), &spec, angle, &out, &args)
        }
        other => Err(format!("unknown command `{other}`\n\n{USAGE}").into()),
    }
}

#[derive(Default)]
struct Parsed {
    out: Option<PathBuf>,
    pages: Option<String>,
    angle: Option<String>,
    /// Password for an encrypted input. Empty by default, which opens the
    /// common case of a file that only carries an owner password.
    password: Option<String>,
    positional: Vec<OsString>,
}

/// One pass over the arguments, so order does not matter and a flag value is
/// never mistaken for a file. `--` ends flag parsing, so a file may be named
/// `-o` or `--pages`.
fn parse_args(args: &[OsString]) -> Res<Parsed> {
    let mut parsed = Parsed::default();
    let mut i = 0;
    let mut flags = true;
    while i < args.len() {
        let arg = &args[i];
        let name = if flags { arg.to_str() } else { None };
        match name {
            Some("--") => {
                flags = false;
                i += 1;
            }
            Some("-o") | Some("--out") => {
                let value = args.get(i + 1).ok_or("-o needs a path")?;
                parsed.out = Some(PathBuf::from(value));
                i += 2;
            }
            Some("--pages") => {
                let value = args.get(i + 1).ok_or("--pages needs a value")?;
                parsed.pages = Some(spec_text(value.clone())?);
                i += 2;
            }
            Some("--angle") => {
                let value = args.get(i + 1).ok_or("--angle needs a value")?;
                parsed.angle = Some(spec_text(value.clone())?);
                i += 2;
            }
            Some("--password") => {
                let value = args.get(i + 1).ok_or("--password needs a value")?;
                parsed.password = Some(value.to_string_lossy().into_owned());
                i += 2;
            }
            _ => {
                parsed.positional.push(arg.clone());
                i += 1;
            }
        }
    }
    Ok(parsed)
}

/// Commands that work on one document reject a second one, so a mistyped flag
/// is an error rather than something silently ignored.
fn one_file(args: &Parsed) -> Res<()> {
    match args.positional.get(1) {
        Some(extra) => Err(format!("unexpected argument `{}`", extra.to_string_lossy()).into()),
        None => Ok(()),
    }
}

/// A page spec is ASCII digits, commas and hyphens, so anything that is not a
/// string cannot be one.
fn spec_text(value: OsString) -> Res<String> {
    value
        .into_string()
        .map_err(|_| "page spec must be plain text".into())
}

/// Load with the path in the message: "IO error" on its own does not say which
/// file was missing.
fn load(path: &Path, password: Option<&str>) -> Res<Document> {
    if path.is_dir() {
        return Err(format!("{}: is a directory", path.display()).into());
    }
    let doc = match password {
        Some(password) => Document::load_with_password(path, password),
        None => Document::load(path),
    }
    .map_err(|err| format!("{}: {err}", path.display()))?;
    // A file whose password is not known still parses: it reports that it is
    // encrypted and yields an empty page tree, which is the failure mode that
    // must not reach a caller.
    if doc.is_encrypted() {
        return Err(format!(
            "{}: this document is encrypted; pass --password with the password it wants",
            path.display()
        )
        .into());
    }
    Ok(doc)
}

/// Everything that rewrites pages needs a document it can actually read. An
/// encrypted file parses far enough to report itself and then yields an empty
/// page tree, so it is decrypted here or it is an error.
///
/// The empty password is tried when none is given: a file that only carries an
/// owner password opens with it, which is the usual case for a document that
/// was "protected" to stop editing rather than to stop reading.
fn load_plain(path: &Path, password: Option<&str>) -> Res<Document> {
    load(path, password)
}

/// Write through a temporary file in the same directory and rename it into
/// place, so a failed write cannot leave a half written document, and writing
/// over the input file cannot destroy it.
fn save(doc: &mut Document, out: &Path) -> Res<()> {
    if let Err(err) = verify(doc) {
        return Err(format!("{}: {err}; nothing was written", out.display()).into());
    }
    let temp = temp_path(out);
    doc.save(&temp)
        .map_err(|err| format!("{}: {err}", out.display()))?;
    fs::rename(&temp, out).map_err(|err| {
        let _ = fs::remove_file(&temp);
        format!("{}: {err}", out.display())
    })?;
    Ok(())
}

/// How many digits a page number needs in a split file name, never fewer than
/// three.
fn page_width(total: u32) -> usize {
    total.to_string().len().max(3)
}

/// Check the document that is about to be written: its catalogue owns a page
/// tree, every reference in it lands on an object, and no stream is missing its
/// length. Walking the model costs milliseconds; parsing the file again to ask
/// the same questions cost a second on a large one.
fn verify(doc: &Document) -> Res<()> {
    let catalog = doc.catalog()?;
    if catalog.get(b"Pages").is_err() {
        return Err("the new catalogue has no page tree".into());
    }
    let mut stack: Vec<&pdfsurgeon::lopdf::Object> = doc.objects.values().collect();
    while let Some(object) = stack.pop() {
        match object {
            pdfsurgeon::lopdf::Object::Reference(id) => {
                if !doc.objects.contains_key(id) {
                    return Err(format!(
                        "the new file points at object {} {}, which is not in it",
                        id.0, id.1
                    )
                    .into());
                }
            }
            pdfsurgeon::lopdf::Object::Dictionary(dict) => {
                stack.extend(dict.iter().map(|(_, value)| value));
            }
            pdfsurgeon::lopdf::Object::Array(items) => stack.extend(items.iter()),
            pdfsurgeon::lopdf::Object::Stream(stream) => {
                if stream.dict.get(b"Length").is_err() {
                    return Err("the new file has a stream with no length".into());
                }
                stack.extend(stream.dict.iter().map(|(_, value)| value));
            }
            _ => {}
        }
    }
    Ok(())
}

fn temp_path(out: &Path) -> PathBuf {
    let name = out
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "out.pdf".to_string());
    out.with_file_name(format!(".{name}.{}.part", std::process::id()))
}

fn cmd_info(path: &Path, args: &Parsed) -> Res<()> {
    let doc = load(path, args.password.as_deref())?;
    // `was_encrypted` is the honest answer once it is open: the input carried
    // encryption, and the output will not.
    let was = doc.was_encrypted();
    let info = pdfsurgeon::info(&doc);
    println!("file:      {}", path.display());
    println!("version:   {}", info.version);
    println!("pages:     {}", info.pages);
    println!(
        "encrypted: {}",
        if was {
            "yes (opened; the output would not be)"
        } else {
            "no"
        }
    );
    if let Some((width, height)) = info.page_size {
        println!("page size: {width:.0} x {height:.0} pt");
    }
    Ok(())
}

fn cmd_merge(inputs: &[OsString], out: &Path, args: &Parsed) -> Res<()> {
    if inputs.is_empty() {
        return Err("merge needs at least one input file".into());
    }
    let mut docs = Vec::with_capacity(inputs.len());
    for input in inputs {
        docs.push(load_plain(Path::new(input), args.password.as_deref())?);
    }
    let mut merged = pdfsurgeon::merge(docs)?;
    save(&mut merged, out)
}

/// Split `file[:pages]` pieces into documents, then assemble them in order.
fn cmd_assemble(inputs: &[OsString], out: &Path, args: &Parsed) -> Res<()> {
    if inputs.is_empty() {
        return Err("assemble needs at least one input file".into());
    }
    let mut sources: Vec<Document> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();

    for input in inputs {
        let (path, spec) = split_piece(input);
        let doc = load_plain(&path, args.password.as_deref())?;
        let total = doc.get_pages().len() as u32;
        let pages = match spec {
            Some(spec) => PageSpec::parse(&spec, total)?.pages().to_vec(),
            None => (1..=total).collect(),
        };
        pieces.push(Piece {
            source: sources.len(),
            pages,
        });
        sources.push(doc);
    }

    let mut doc = pdfsurgeon::assemble(&sources, &pieces)?;
    save(&mut doc, out)
}

/// `report.pdf:1-3` is a path and a page spec. `C:\report.pdf` and
/// `odd:name.pdf` are just paths, so only a suffix that reads as a spec is
/// taken as one.
fn split_piece(piece: &OsString) -> (PathBuf, Option<String>) {
    if let Some(text) = piece.to_str() {
        if let Some((path, spec)) = text.rsplit_once(':') {
            if !path.is_empty() && looks_like_spec(spec) {
                return (PathBuf::from(path), Some(spec.to_string()));
            }
        }
    }
    (PathBuf::from(piece), None)
}

fn looks_like_spec(text: &str) -> bool {
    !text.is_empty()
        && text.chars().any(|c| c.is_ascii_digit())
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == '-' || c == ' ')
}

fn cmd_split(path: &Path, out_dir: Option<PathBuf>, args: &Parsed) -> Res<()> {
    let source = load_plain(path, args.password.as_deref())?;
    let total = source.get_pages().len() as u32;
    let dir = out_dir.unwrap_or_else(|| {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    });
    fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    if !dir.is_dir() {
        return Err(format!("{}: not a directory", dir.display()).into());
    }
    let stem = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "page".to_string());

    // Three digits reads well up to a few hundred pages; a document with more
    // pages than that gets the width it needs, so the files still sort.
    let width = page_width(total);
    pdfsurgeon::split_into(&source, |page, mut single| {
        let name = format!("{stem}-{page:0width$}.pdf");
        save(&mut single, &dir.join(name)).map_err(|err| std::io::Error::other(err.to_string()))
    })?;
    Ok(())
}

fn cmd_select(path: &Path, spec: &str, out: &Path, args: &Parsed) -> Res<()> {
    let source = load_plain(path, args.password.as_deref())?;
    let total = source.get_pages().len() as u32;
    let keep = PageSpec::parse(spec, total)?;
    let mut doc = pdfsurgeon::select(&source, keep.pages())?;
    save(&mut doc, out)
}

fn cmd_rotate(path: &Path, spec: &str, angle: i64, out: &Path, args: &Parsed) -> Res<()> {
    let mut doc = load_plain(path, args.password.as_deref())?;
    let total = doc.get_pages().len() as u32;
    let pages = PageSpec::parse(spec, total)?;
    pdfsurgeon::rotate(&mut doc, pages.pages(), angle)?;
    save(&mut doc, out)
}

#[cfg(test)]
mod tests {
    use super::page_width;

    #[test]
    fn split_names_keep_three_digits_until_the_page_count_needs_more() {
        assert_eq!(page_width(1), 3);
        assert_eq!(page_width(194), 3);
        assert_eq!(page_width(999), 3);
        assert_eq!(page_width(1000), 4);
        assert_eq!(page_width(25_000), 5);
    }
}
