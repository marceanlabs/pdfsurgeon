//! Page level surgery for PDF files.
//!
//! The library mirrors the command line tool: it never edits pages in place and
//! never deletes objects from the input. Anything that removes pages builds a
//! new document holding only the objects the kept pages can reach, with every
//! reference renumbered. A dangling destination therefore cannot exist in the
//! output, and readers do not warn about the result.
//!
//! ```no_run
//! use pdfsurgeon::{Document, PageSpec};
//!
//! # fn main() -> Result<(), pdfsurgeon::Error> {
//! let doc = Document::load("report.pdf")?;
//! let keep = PageSpec::parse("1-3,7", doc.get_pages().len() as u32)?;
//! let mut trimmed = pdfsurgeon::select(&doc, keep.pages())?;
//! trimmed.save("extract.pdf")?;
//! # Ok(())
//! # }
//! ```
//!
//! `merge` keeps everything from every input and builds one page tree over all
//! of them, so links between the merged files' own pages keep working.
//! `assemble` is the general case: pages from any number of inputs, in the
//! order listed.

use std::collections::BTreeMap;
use std::fmt;

pub use lopdf;
pub use lopdf::Document;
use lopdf::{Object, ObjectId, dictionary};

/// Everything this crate can go wrong with.
#[derive(Debug)]
pub enum Error {
    /// The underlying PDF library refused the file or the operation.
    Pdf(lopdf::Error),
    /// Reading or writing a file failed.
    Io(std::io::Error),
    /// A page specification could not be understood.
    Spec(String),
    /// A page number was outside `1..=total`.
    Page { number: u32, total: u32 },
    /// A page number that exists in the tree but could not be resolved.
    MissingPage(u32),
    /// A rotation that is not a whole number of right angles.
    Angle(i64),
    /// The call needs at least one page or one input.
    Empty,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Pdf(err) => write!(f, "{err}"),
            Error::Io(err) => write!(f, "{err}"),
            Error::Spec(reason) => write!(f, "{reason}"),
            Error::Page { number, total } => {
                write!(f, "page {number} is outside 1..={total}")
            }
            Error::MissingPage(number) => write!(f, "page {number} is missing"),
            Error::Angle(angle) => write!(f, "angle {angle} is not a multiple of 90"),
            Error::Empty => write!(f, "at least one page is required"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Pdf(err) => Some(err),
            Error::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<lopdf::Error> for Error {
    fn from(err: lopdf::Error) -> Self {
        Error::Pdf(err)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Io(err)
    }
}

/// The result type used throughout this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// A parsed page specification such as `1`, `1-4` or `1,3-5,9`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSpec {
    pages: Vec<u32>,
}

impl PageSpec {
    /// Parse `spec` against a document with `total` pages.
    ///
    /// Numbers are one based and inclusive; ranges keep the order written, so
    /// `3,1-2` is `[3, 1, 2]`.
    pub fn parse(spec: &str, total: u32) -> Result<Self> {
        let mut pages = Vec::new();
        for part in spec.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            match part.split_once('-') {
                Some((from, to)) => {
                    let from: u32 = from
                        .trim()
                        .parse()
                        .map_err(|_| Error::Spec(format!("bad range `{part}`")))?;
                    let to: u32 = to
                        .trim()
                        .parse()
                        .map_err(|_| Error::Spec(format!("bad range `{part}`")))?;
                    if from == 0 || to < from {
                        return Err(Error::Spec(format!("bad range `{part}`")));
                    }
                    pages.extend(from..=to);
                }
                None => pages.push(
                    part.parse()
                        .map_err(|_| Error::Spec(format!("bad page `{part}`")))?,
                ),
            }
        }
        if pages.is_empty() {
            return Err(Error::Spec("page spec is empty".to_string()));
        }
        if let Some(bad) = pages.iter().find(|n| **n == 0 || **n > total) {
            return Err(Error::Page {
                number: *bad,
                total,
            });
        }
        Ok(PageSpec { pages })
    }

    /// The page numbers, in the order they were written.
    pub fn pages(&self) -> &[u32] {
        &self.pages
    }
}

/// What `pdfsurgeon info` reports.
#[derive(Debug, Clone, PartialEq)]
pub struct Info {
    /// PDF version from the header, for example `1.7`.
    pub version: String,
    /// Number of pages in the page tree.
    pub pages: usize,
    /// Whether the document is encrypted.
    pub encrypted: bool,
    /// Size of the first page in points, when the media box is readable.
    pub page_size: Option<(f32, f32)>,
}

/// Read the structure of a document without touching the page content.
pub fn info(doc: &Document) -> Info {
    let pages = doc.get_pages();
    let page_size = pages
        .values()
        .next()
        .and_then(|id| doc.get_dictionary(*id).ok())
        .and_then(|dict| dict.get(b"MediaBox").ok().and_then(|o| o.as_array().ok()))
        .and_then(|media| {
            let nums: Vec<f32> = media.iter().filter_map(|o| o.as_float().ok()).collect();
            (nums.len() == 4).then(|| (nums[2] - nums[0], nums[3] - nums[1]))
        });
    Info {
        version: doc.version.clone(),
        pages: pages.len(),
        encrypted: doc.is_encrypted(),
        page_size,
    }
}

/// Build a new document containing only `pages` of `doc`, in the order given.
///
/// Page numbers are one based. The result holds only what those pages can
/// reach, so it cannot contain a dangling reference.
pub fn select(doc: &Document, pages: &[u32]) -> Result<Document> {
    if pages.is_empty() {
        return Err(Error::Empty);
    }
    let by_number = doc.get_pages();
    let total = by_number.len() as u32;
    let mut ids = Vec::with_capacity(pages.len());
    for number in pages {
        let id = *by_number.get(number).ok_or(Error::Page {
            number: *number,
            total,
        })?;
        ids.push(id);
    }
    let keys: Vec<(usize, ObjectId)> = ids.iter().map(|id| (0usize, *id)).collect();
    Ok(extract_from(std::slice::from_ref(doc), &keys))
}

/// One slice of an [`assemble`] call: pages from the input at `source`, in the
/// order given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// Index into the slice of documents passed to [`assemble`].
    pub source: usize,
    /// Pages to take from that document, one based.
    pub pages: Vec<u32>,
}

impl Piece {
    /// Every page of the document at `source`.
    pub fn all(source: usize, total: u32) -> Self {
        Piece {
            source,
            pages: (1..=total).collect(),
        }
    }
}

/// Build a new document from pieces of several inputs, in the order listed.
///
/// This is the general case of both [`merge`] and [`select`].
pub fn assemble(sources: &[Document], pieces: &[Piece]) -> Result<Document> {
    if pieces.is_empty() {
        return Err(Error::Empty);
    }
    let maps: Vec<BTreeMap<u32, ObjectId>> = sources.iter().map(Document::get_pages).collect();
    let mut keys: Vec<(usize, ObjectId)> = Vec::new();
    for piece in pieces {
        let map = maps.get(piece.source).ok_or(Error::Spec(format!(
            "no input {} in this assemble",
            piece.source
        )))?;
        let total = map.len() as u32;
        for number in &piece.pages {
            let id = *map.get(number).ok_or(Error::Page {
                number: *number,
                total,
            })?;
            keys.push((piece.source, id));
        }
    }
    if keys.is_empty() {
        return Err(Error::Empty);
    }
    Ok(extract_from(sources, &keys))
}

/// Every page of every input, in order.
pub fn merge(sources: &[Document]) -> Result<Document> {
    if sources.is_empty() {
        return Err(Error::Empty);
    }
    let pieces: Vec<Piece> = sources
        .iter()
        .enumerate()
        .map(|(index, doc)| Piece::all(index, doc.get_pages().len() as u32))
        .collect();
    assemble(sources, &pieces)
}

/// Add `angle` degrees to the rotation of `pages`, in place.
///
/// The angle must be a multiple of 90 and accumulates with any rotation the
/// page already carries, so rotating twice by 90 leaves the page at 180.
pub fn rotate(doc: &mut Document, pages: &[u32], angle: i64) -> Result<()> {
    if angle % 90 != 0 {
        return Err(Error::Angle(angle));
    }
    if pages.is_empty() {
        return Err(Error::Empty);
    }
    let by_number = doc.get_pages();
    let total = by_number.len() as u32;
    for number in pages {
        let id = *by_number.get(number).ok_or(Error::Page {
            number: *number,
            total,
        })?;
        let dict = doc.get_dictionary_mut(id)?;
        let current = dict
            .get(b"Rotate")
            .ok()
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        let updated = ((current + angle) % 360 + 360) % 360;
        dict.set("Rotate", updated);
    }
    Ok(())
}

/// Takes pages from several documents and builds a new one holding only what
/// those pages can reach. Nothing else travels over, so no dangling reference
/// can exist in the result.
fn extract_from(sources: &[Document], keys: &[(usize, ObjectId)]) -> Document {
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

    for key in keys {
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

    let kids: Vec<Object> = keys.iter().map(|key| Object::Reference(map[key])).collect();
    out.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => keys.len() as i64,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(labels: &[&str]) -> Document {
        use lopdf::{
            Stream,
            content::{Content, Operation},
            dictionary,
        };
        let mut d = Document::with_version("1.5");
        let pages_id = d.new_object_id();
        let font_id = d.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica",
        });
        let resources_id = d.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let mut kids = Vec::new();
        for label in labels {
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 24.into()]),
                    Operation::new("Td", vec![72.into(), 720.into()]),
                    Operation::new("Tj", vec![Object::string_literal(*label)]),
                    Operation::new("ET", vec![]),
                ],
            };
            let content_id = d.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
            let page_id = d.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            });
            kids.push(page_id);
        }
        d.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => kids.into_iter().map(Object::Reference).collect::<Vec<_>>(),
                "Count" => labels.len() as i64,
            }),
        );
        let catalog = d.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        d.trailer.set("Root", catalog);
        d
    }

    #[test]
    fn spec_parses_ranges_and_lists() {
        let spec = PageSpec::parse("1-3,7", 10).unwrap();
        assert_eq!(spec.pages(), &[1, 2, 3, 7]);
        let spec = PageSpec::parse("3,1-2", 10).unwrap();
        assert_eq!(spec.pages(), &[3, 1, 2]);
    }

    #[test]
    fn spec_rejects_out_of_range_and_empty() {
        assert!(matches!(PageSpec::parse("0", 5), Err(Error::Page { .. })));
        assert!(matches!(PageSpec::parse("3-1", 5), Err(Error::Spec(_))));
        assert!(matches!(PageSpec::parse("9", 5), Err(Error::Page { .. })));
        assert!(matches!(PageSpec::parse("", 5), Err(Error::Spec(_))));
    }

    #[test]
    fn select_keeps_only_the_asked_pages() {
        let source = doc(&["alpha", "beta", "gamma"]);
        let out = select(&source, &[3]).unwrap();
        assert_eq!(out.get_pages().len(), 1);
        assert_eq!(out.extract_text(&[1]).unwrap().trim(), "gamma");
    }

    #[test]
    fn merge_keeps_page_order_and_rewrites_ids() {
        let a = doc(&["a1", "a2"]);
        let b = doc(&["b1"]);
        let merged = merge(&[a, b]).unwrap();
        assert_eq!(merged.get_pages().len(), 3);
        assert_eq!(merged.extract_text(&[1]).unwrap().trim(), "a1");
        assert_eq!(merged.extract_text(&[3]).unwrap().trim(), "b1");
    }

    #[test]
    fn assemble_takes_pages_in_the_order_listed() {
        let a = doc(&["a1", "a2", "a3"]);
        let b = doc(&["b1", "b2"]);
        let out = assemble(
            &[a, b],
            &[
                Piece {
                    source: 1,
                    pages: vec![2],
                },
                Piece {
                    source: 0,
                    pages: vec![3, 1],
                },
            ],
        )
        .unwrap();
        assert_eq!(out.get_pages().len(), 3);
        assert_eq!(out.extract_text(&[1]).unwrap().trim(), "b2");
        assert_eq!(out.extract_text(&[3]).unwrap().trim(), "a1");
    }

    #[test]
    fn rotate_accumulates_and_rejects_other_angles() {
        let mut source = doc(&["p1", "p2"]);
        rotate(&mut source, &[1], 90).unwrap();
        rotate(&mut source, &[1], 90).unwrap();
        let id = source.get_pages()[&1];
        let dict = source.get_dictionary(id).unwrap();
        assert_eq!(dict.get(b"Rotate").unwrap().as_i64().unwrap(), 180);
        assert!(matches!(
            rotate(&mut source, &[1], 45),
            Err(Error::Angle(45))
        ));
    }

    #[test]
    fn info_reports_pages_and_size() {
        let source = doc(&["one", "two"]);
        let info = info(&source);
        assert_eq!(info.pages, 2);
        assert_eq!(info.version, "1.5");
        assert_eq!(info.page_size, Some((595.0, 842.0)));
        assert!(!info.encrypted);
    }
}
