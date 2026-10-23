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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub use lopdf;
pub use lopdf::Document;
use lopdf::{Dictionary, Object, ObjectId, dictionary};

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
    /// The document is encrypted, so its pages cannot be rewritten faithfully.
    Encrypted,
    /// A stream in the input is not the length its dictionary claims, which
    /// means the file is damaged and copying it would quietly lose content.
    Damaged(ObjectId),
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
            Error::Encrypted => write!(f, "document is encrypted"),
            Error::Damaged(id) => write!(
                f,
                "object {} {} is damaged: its stream is not the length its dictionary claims",
                id.0, id.1
            ),
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

/// Attributes a page takes from its ancestors when it does not carry them
/// itself (PDF 32000-1, table 30). A page copied without them opens with no
/// fonts, no size, or the wrong rotation.
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// The value `key` resolves to for `page`: the page's own value wins, then each
/// `/Parent` in turn. `/Parent` cycles exist in broken files, so the walk
/// remembers where it has been.
fn inherited(doc: &Document, page: ObjectId, key: &[u8]) -> Option<Object> {
    let mut seen = BTreeSet::new();
    let mut current = Some(page);
    while let Some(id) = current {
        if !seen.insert(id) {
            return None;
        }
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(value.clone());
        }
        current = dict
            .get(b"Parent")
            .ok()
            .and_then(|parent| parent.as_reference().ok());
    }
    None
}

/// Every inheritable attribute `page` does not carry itself, resolved and
/// ready to be set on a copy of that page.
fn inherited_missing(doc: &Document, page: ObjectId) -> Vec<(&'static [u8], Object)> {
    let Ok(dict) = doc.get_dictionary(page) else {
        return Vec::new();
    };
    let mut missing = Vec::new();
    for key in INHERITABLE {
        if dict.get(key).is_ok() {
            continue;
        }
        if let Some(value) = inherited(doc, page, key) {
            missing.push((key, value));
        }
    }
    missing
}

/// Refuse a document whose streams do not hold what their dictionaries say.
///
/// A parser that recovers from a corrupt `/Length` can hand back an object with
/// no content at all; writing that out would produce a document that opens and
/// is missing a font or an image, which is worse than refusing the file.
fn check_streams(doc: &Document) -> Result<()> {
    for (id, object) in &doc.objects {
        let Object::Stream(stream) = object else {
            continue;
        };
        // A stream must say how long it is, directly or through a reference,
        // and the number must match the bytes that were read. Anything else
        // means the parser recovered the object by guessing, and a guess is
        // not something to copy into a new file.
        let Some(declared) = stream_length(doc, stream) else {
            return Err(Error::Damaged(*id));
        };
        if declared < 0 || declared as usize != stream.content.len() {
            return Err(Error::Damaged(*id));
        }
    }
    Ok(())
}

fn stream_length(doc: &Document, stream: &lopdf::Stream) -> Option<i64> {
    match stream.dict.get(b"Length").ok()? {
        Object::Integer(value) => Some(*value),
        Object::Reference(id) => doc.get_object(*id).ok()?.as_i64().ok(),
        _ => None,
    }
}

/// Follow a reference one step; anything else is its own value.
fn follow<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Object> {
    match object {
        Object::Reference(id) => doc.get_object(*id).ok(),
        other => Some(other),
    }
}

/// Every named destination the document defines, name to destination.
///
/// The name tree under `/Names /Dests` is the current form; `/Dests` on the
/// catalogue is the older one. Both are read, because files from the last
/// twenty years contain both.
fn named_destinations(doc: &Document) -> BTreeMap<Vec<u8>, Object> {
    let mut names = BTreeMap::new();
    let Ok(catalog) = doc.catalog() else {
        return names;
    };
    if let Ok(tree) = catalog.get(b"Names")
        && let Some(dict) = follow(doc, tree).and_then(|node| node.as_dict().ok())
        && let Ok(dests) = dict.get(b"Dests")
    {
        collect_name_tree(doc, dests, &mut names, 0);
    }
    if let Ok(dests) = catalog.get(b"Dests")
        && let Some(dict) = follow(doc, dests).and_then(|node| node.as_dict().ok())
    {
        for (name, value) in dict.iter() {
            names
                .entry(name.clone())
                .or_insert_with(|| unwrap_reference(doc, value, 0));
        }
    }
    names
}

fn collect_name_tree(
    doc: &Document,
    node: &Object,
    names: &mut BTreeMap<Vec<u8>, Object>,
    depth: usize,
) {
    // A name tree can nest; a damaged one can nest forever.
    if depth > 32 {
        return;
    }
    let Some(dict) = follow(doc, node).and_then(|node| node.as_dict().ok()) else {
        return;
    };
    if let Ok(entries) = dict.get(b"Names")
        && let Ok(entries) = entries.as_array()
    {
        for pair in entries.chunks(2) {
            if let [key, value] = pair
                && let Ok(name) = key.as_str()
            {
                names.insert(name.to_vec(), unwrap_reference(doc, value, 0));
            }
        }
    }
    if let Ok(kids) = dict.get(b"Kids")
        && let Ok(kids) = kids.as_array()
    {
        for kid in kids {
            collect_name_tree(doc, kid, names, depth + 1);
        }
    }
}

/// Follow a chain of references to the object it lands on.
fn unwrap_reference(doc: &Document, value: &Object, depth: usize) -> Object {
    match value {
        Object::Reference(id) if depth < 8 => match doc.get_object(*id) {
            Ok(inner) => unwrap_reference(doc, inner, depth + 1),
            Err(_) => value.clone(),
        },
        other => other.clone(),
    }
}

/// A destination as an explicit array of page reference plus view, following
/// names until it reaches one. `None` means the name is not defined here.
fn resolve_destination(
    value: &Object,
    names: &BTreeMap<Vec<u8>, Object>,
    depth: usize,
) -> Option<Object> {
    if depth > 8 {
        return None;
    }
    match value {
        Object::Array(_) => Some(value.clone()),
        Object::Dictionary(_) => {
            let inner = value.as_dict().ok()?.get(b"D").ok()?;
            resolve_destination(inner, names, depth + 1)
        }
        Object::String(..) | Object::Name(..) => {
            let key = value.as_str().ok()?.to_vec();
            resolve_destination(names.get(&key)?, names, depth + 1)
        }
        _ => None,
    }
}

fn target_is_kept(destination: &Object, kept: Option<&BTreeSet<ObjectId>>) -> bool {
    let Some(kept) = kept else {
        return true;
    };
    let Ok(items) = destination.as_array() else {
        return false;
    };
    match items.first() {
        Some(Object::Reference(id)) => kept.contains(id),
        _ => false,
    }
}

/// Point a link at the page it names instead of at a name.
///
/// A name tree lives in the catalogue, and a rebuilt catalogue does not carry
/// one, so a `/A << /S /GoTo /D (section.2) >>` would otherwise point at
/// nothing. When the page it names is not part of the output, the destination
/// is dropped rather than left dangling: the link stops working, which is what
/// the document says, instead of opening somewhere arbitrary.
fn resolve_links(
    object: &mut Object,
    names: &BTreeMap<Vec<u8>, Object>,
    kept: Option<&BTreeSet<ObjectId>>,
) {
    let Object::Dictionary(dict) = object else {
        return;
    };
    if let Ok(destination) = dict.get(b"Dest").cloned() {
        match resolve_destination(&destination, names, 0)
            .filter(|array| target_is_kept(array, kept))
        {
            Some(array) => dict.set("Dest", array),
            None => {
                dict.remove(b"Dest");
            }
        }
    }
    let goto = dict.get(b"S").ok().and_then(|kind| kind.as_name().ok()) == Some(b"GoTo");
    if goto && let Ok(destination) = dict.get(b"D").cloned() {
        match resolve_destination(&destination, names, 0)
            .filter(|array| target_is_kept(array, kept))
        {
            Some(array) => dict.set("D", array),
            None => {
                dict.remove(b"D");
                dict.remove(b"S");
            }
        }
    }
}

/// A page box number, whether the producer wrote it as an integer or a real.
fn as_number(object: &Object) -> Option<f32> {
    match object {
        Object::Integer(value) => Some(*value as f32),
        Object::Real(value) => Some(*value),
        _ => None,
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
                    // Bound the range before expanding it: `1-4000000000`
                    // against a four page file must be a page error, not a
                    // sixteen gigabyte allocation.
                    if from > total {
                        return Err(Error::Page {
                            number: from,
                            total,
                        });
                    }
                    if to > total {
                        return Err(Error::Page { number: to, total });
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
    let page_size = pages.values().next().and_then(|id| {
        let media = inherited(doc, *id, b"MediaBox")?;
        let media = media.as_array().ok()?;
        let nums: Vec<f32> = media.iter().filter_map(as_number).collect();
        (nums.len() == 4).then(|| (nums[2] - nums[0], nums[3] - nums[1]))
    });
    Info {
        version: doc.version.clone(),
        pages: pages.len(),
        encrypted: doc.is_encrypted(),
        page_size,
    }
}

/// One document per page, in page order.
///
/// The same work as calling [`select`] once per page, but the name tree and the
/// stream check are read once instead of once per page, which is most of the
/// cost on a large file.
pub fn split(doc: &Document) -> Result<Vec<Document>> {
    if doc.is_encrypted() {
        return Err(Error::Encrypted);
    }
    check_streams(doc)?;
    let by_number = doc.get_pages();
    let names = named_destinations(doc);
    let names = std::slice::from_ref(&names);
    let source = std::slice::from_ref(doc);
    let mut pages = Vec::with_capacity(by_number.len());
    for id in by_number.values() {
        // No bookmarks in a one page file: the only ones left would point at
        // the page they are listed in, and walking the outline tree once per
        // page is a quarter of the time split spends.
        pages.push(extract_with_names(source, &[(0, *id)], names, false));
    }
    Ok(pages)
}

/// Build a new document containing only `pages` of `doc`, in the order given.
///
/// Page numbers are one based. The result holds only what those pages can
/// reach, so it cannot contain a dangling reference.
pub fn select(doc: &Document, pages: &[u32]) -> Result<Document> {
    if pages.is_empty() {
        return Err(Error::Empty);
    }
    if doc.is_encrypted() {
        return Err(Error::Encrypted);
    }
    check_streams(doc)?;
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
    if sources.iter().any(Document::is_encrypted) {
        return Err(Error::Encrypted);
    }
    for source in sources {
        check_streams(source)?;
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
///
/// Consumes the inputs: their objects are moved into the result rather than
/// copied, which is what keeps a large merge cheap.
pub fn merge(sources: Vec<Document>) -> Result<Document> {
    if sources.is_empty() {
        return Err(Error::Empty);
    }
    if sources.iter().any(Document::is_encrypted) {
        return Err(Error::Encrypted);
    }
    for source in &sources {
        check_streams(source)?;
    }
    // The document level bits worth carrying from the first input: its
    // metadata, and the view it asked a reader to open in.
    let single_input = sources.len() == 1;
    let first_info = sources[0].trailer.get(b"Info").ok().cloned();
    let first_catalog = sources[0].catalog().ok().cloned();
    let first_open_action = first_catalog
        .as_ref()
        .and_then(|catalog| catalog.get(b"OpenAction").ok().cloned())
        .filter(|action| match action {
            // Only a plain page destination, whose page keeps its id because
            // the first input is copied without an offset.
            Object::Array(items) => matches!(items.first(), Some(Object::Reference(_))),
            Object::Reference(id) => sources[0].get_dictionary(*id).is_ok(),
            _ => false,
        });
    let first_page_mode = first_catalog
        .as_ref()
        .and_then(|catalog| catalog.get(b"PageMode").ok())
        .and_then(|mode| mode.as_name().ok())
        .map(<[u8]>::to_vec);

    let mut out = Document::with_version(sources[0].version.clone());
    let mut offset: u32 = 0;
    let mut page_ids: Vec<ObjectId> = Vec::new();
    // Top level chains of each source outline, in output ids.
    let mut outlines: Vec<(ObjectId, i64)> = Vec::new();
    let mut forms: Vec<SourceForm> = Vec::new();

    for doc in sources {
        let by_number = doc.get_pages();
        // Where the next document's ids start. Taken from the objects that are
        // actually there, never from `max_id`: a damaged trailer can claim four
        // hundred million objects, and shifting by that writes a file no reader
        // will open.
        let span = doc
            .objects
            .keys()
            .map(|(number, _)| *number as u64)
            .max()
            .unwrap_or(0)
            + 1;
        if span > 10_000_000 || offset as u64 + span > u32::MAX as u64 {
            return Err(Error::Damaged((span.min(u32::MAX as u64) as u32, 0)));
        }
        let span = span as u32;
        page_ids.extend(by_number.values().map(|(n, g)| (n + offset, *g)));

        let names = named_destinations(&doc);
        if let Some(form) = doc
            .catalog()
            .ok()
            .and_then(|catalog| catalog.get(b"AcroForm").ok().cloned())
            && let Some(form) = read_source_form(&doc, offset, form)
        {
            forms.push(form);
        }
        if let Some(dict) = doc
            .catalog()
            .ok()
            .and_then(|catalog| catalog.get(b"Outlines").ok())
            .and_then(|outlines| outlines.as_reference().ok())
            .and_then(|id| doc.get_dictionary(id).ok())
        {
            let first = dict.get(b"First").ok().and_then(|o| o.as_reference().ok());
            let count = dict
                .get(b"Count")
                .ok()
                .and_then(|o| o.as_i64().ok())
                .unwrap_or(0);
            if let Some(first) = first {
                outlines.push(((first.0 + offset, first.1), count));
            }
        }

        // Pages keep their own dictionaries, but the tree they used to hang
        // from is thrown away, so anything they inherited from it has to be
        // written onto them here or the page opens without fonts or a size.
        let mut carried: BTreeMap<ObjectId, Vec<(&'static [u8], Object)>> = BTreeMap::new();
        for id in by_number.values() {
            let missing = inherited_missing(&doc, *id);
            if !missing.is_empty() {
                carried.insert(*id, missing);
            }
        }

        for (id, mut object) in doc.objects {
            match object.type_name().unwrap_or(b"") {
                // The output owns one page tree and one catalogue, and it
                // writes its own cross reference table: a source's `/XRef`
                // stream describes the byte layout of a file that no longer
                // exists, and points at the catalogue we just replaced.
                b"Catalog" | b"Pages" | b"XRef" => continue,
                _ => {}
            }
            if let Some(attributes) = carried.get(&id) {
                if let Object::Dictionary(dict) = &mut object {
                    for (key, value) in attributes {
                        dict.set(*key, value.clone());
                    }
                }
            }
            resolve_links(&mut object, &names, None);
            shift_refs(&mut object, offset);
            out.objects.insert((id.0 + offset, id.1), object);
        }
        offset += span;
    }

    // Every page needs the new tree as its parent, and ids were inserted
    // directly, so the next free id comes from the map.
    out.max_id = out.objects.keys().map(|(n, _)| *n).max().unwrap_or(0);
    let pages_id = (out.max_id + 1, 0);
    out.max_id += 1;
    for id in &page_ids {
        if let Ok(dict) = out.get_dictionary_mut(*id) {
            dict.set("Parent", pages_id);
        }
    }

    let kids: Vec<Object> = page_ids.iter().map(|id| Object::Reference(*id)).collect();
    let count = kids.len() as i64;
    out.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => count,
        }),
    );

    let mut catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    };
    if let Some(outlines_id) = rebuild_outlines(&mut out, &outlines) {
        // A merged document that has bookmarks should offer them: the reader
        // only shows the panel when the catalogue asks it to.
        if first_page_mode.as_deref() == Some(b"UseOutlines") {
            catalog.set("PageMode", "UseOutlines");
        }
        catalog.set("Outlines", outlines_id);
    }
    if let Some(action) = first_open_action {
        catalog.set("OpenAction", action);
    }
    if single_input
        && let Some(metadata) = first_catalog
            .as_ref()
            .and_then(|catalog| catalog.get(b"Metadata").ok())
        && metadata.as_reference().is_ok()
    {
        catalog.set("Metadata", metadata.clone());
    }
    if let Some(acroform) = merge_forms(&mut out, &forms) {
        catalog.set("AcroForm", acroform);
    }
    let catalog_id = out.add_object(catalog);
    out.trailer.set("Root", catalog_id);
    if let Some(info) = first_info {
        out.trailer.set("Info", info);
    }
    Ok(out)
}

/// Splice the top level of every source outline into one tree.
///
/// The items themselves are ordinary objects and were copied with the rest; all
/// that is missing is a root that owns them, the links between the chains, and
/// a `/Parent` pointing at that root instead of at the old one.
fn rebuild_outlines(out: &mut Document, chains: &[(ObjectId, i64)]) -> Option<ObjectId> {
    let mut valid: Vec<(ObjectId, ObjectId, i64)> = Vec::new();
    for (first, count) in chains {
        if !out.objects.contains_key(first) {
            continue;
        }
        let mut current = Some(*first);
        let mut previous: Option<ObjectId> = None;
        while let Some(id) = current {
            let next = out
                .get_dictionary(id)
                .ok()
                .and_then(|dict| dict.get(b"Next").ok())
                .and_then(|next| next.as_reference().ok());
            previous = Some(id);
            current = next;
        }
        if let Some(last) = previous {
            valid.push((*first, last, *count));
        }
    }
    if valid.is_empty() {
        return None;
    }

    let root = (out.max_id + 1, 0);
    out.max_id += 1;
    let mut total = 0i64;
    let mut previous_last: Option<ObjectId> = None;
    for (first, last, count) in &valid {
        if let Ok(dict) = out.get_dictionary_mut(*first) {
            dict.remove(b"Prev");
            dict.set("Parent", root);
        }
        if let Ok(dict) = out.get_dictionary_mut(*last) {
            dict.remove(b"Next");
            dict.set("Parent", root);
        }
        if let Some(previous) = previous_last {
            if let Ok(dict) = out.get_dictionary_mut(previous) {
                dict.set("Next", *first);
            }
            if let Ok(dict) = out.get_dictionary_mut(*first) {
                dict.set("Prev", previous);
            }
        }
        previous_last = Some(*last);
        total += *count;
    }

    let first = valid[0].0;
    let last = previous_last?;
    // Every top level item needs the new root as its parent, not just the ends.
    let mut current = Some(first);
    while let Some(id) = current {
        let next = out
            .get_dictionary(id)
            .ok()
            .and_then(|dict| dict.get(b"Next").ok())
            .and_then(|next| next.as_reference().ok());
        if let Ok(dict) = out.get_dictionary_mut(id) {
            dict.set("Parent", root);
        }
        current = next;
    }
    out.objects.insert(
        root,
        Object::Dictionary(dictionary! {
            "Type" => "Outlines",
            "First" => first,
            "Last" => last,
            "Count" => total,
        }),
    );
    Some(root)
}

/// Add `offset` to every object number inside `object`, so references keep
/// pointing at the same objects after their ids have moved.
fn shift_refs(object: &mut Object, offset: u32) {
    if offset == 0 {
        return;
    }
    match object {
        Object::Reference(id) => id.0 += offset,
        Object::Dictionary(dict) => {
            for (_, value) in dict.iter_mut() {
                shift_refs(value, offset);
            }
        }
        Object::Array(items) => {
            for item in items.iter_mut() {
                shift_refs(item, offset);
            }
        }
        Object::Stream(stream) => {
            for (_, value) in stream.dict.iter_mut() {
                shift_refs(value, offset);
            }
        }
        _ => {}
    }
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
    if doc.is_encrypted() {
        return Err(Error::Encrypted);
    }
    check_streams(doc)?;
    let by_number = doc.get_pages();
    let total = by_number.len() as u32;
    for number in pages {
        let id = *by_number.get(number).ok_or(Error::Page {
            number: *number,
            total,
        })?;
        // A page that inherits its rotation from a Pages node still has that
        // rotation in a reader, so the new value starts from what it inherits.
        let current = inherited(doc, id, b"Rotate")
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        let dict = doc.get_dictionary_mut(id)?;
        let updated = (current.rem_euclid(360) + angle.rem_euclid(360)) % 360;
        dict.set("Rotate", updated);
    }
    Ok(())
}

/// Takes pages from several documents and builds a new one holding only what
/// those pages can reach. Nothing else travels over, so no dangling reference
/// can exist in the result.
fn extract_from(sources: &[Document], keys: &[(usize, ObjectId)]) -> Document {
    let names: Vec<BTreeMap<Vec<u8>, Object>> = sources.iter().map(named_destinations).collect();
    extract_with_names(sources, keys, &names, true)
}

/// The body of [`extract_from`], with the named destinations already read so a
/// caller that extracts many times from one document pays for them once.
fn extract_with_names(
    sources: &[Document],
    keys: &[(usize, ObjectId)],
    names: &[BTreeMap<Vec<u8>, Object>],
    bookmarks: bool,
) -> Document {
    let version = sources
        .first()
        .map(|d| d.version.clone())
        .unwrap_or_else(|| "1.5".to_string());
    let mut out = Document::with_version(version);
    let mut kept_by_source: BTreeMap<usize, BTreeSet<ObjectId>> = BTreeMap::new();
    for (source, id) in keys {
        kept_by_source.entry(*source).or_default().insert(*id);
    }
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
            && is_widget(dict)
        {
            // The field above this widget lives in the catalogue being
            // replaced. Keeping the pointer would drag the whole form, and any
            // page its other widgets sit on, into a document that does not have
            // those pages.
            dict.remove(b"Parent");
        }
        if let Object::Dictionary(dict) = &mut object
            && dict.get(b"Type").ok().and_then(|t| t.as_name().ok()) == Some(b"Page")
        {
            // The parent is provided by the new page tree, not copied.
            dict.remove(b"Parent");
            // The new tree is flat, so anything the page inherited from the
            // old one has to be written onto the page itself or it is lost.
            for (key, value) in inherited_missing(&sources[key.0], key.1) {
                dict.set(key, value);
            }
        }
        // Before the references are remapped, turn any named destination into
        // the page it names, and drop it if that page is not coming along.
        resolve_links(&mut object, &names[key.0], kept_by_source.get(&key.0));
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

    // Each copied page was stripped of its old parent, so the new tree has to
    // adopt it: a page without /Parent is not a valid page.
    for key in keys {
        if let Ok(dict) = out.get_dictionary_mut(map[key]) {
            dict.set("Parent", pages_id);
        }
    }

    out.max_id = next - 1;
    if let Some(info) = self_contained_info(&sources[0]) {
        let info = out.add_object(info);
        out.trailer.set("Info", info);
    }
    let outlines = if bookmarks {
        build_outlines(&mut out, &sources[0], &names[0], &map)
    } else {
        None
    };
    let acroform = build_extracted_form(sources, &mut out, &map, keys);
    // XMP describes the document it came from. Extracting pages from one file
    // keeps the description true enough; a merge of several would make it a
    // statement about a document that never existed.
    let metadata = if sources.len() == 1 {
        copy_metadata(&sources[0], &mut out)
    } else {
        None
    };
    let mut catalog = dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
    };
    if let Some(acroform) = acroform {
        catalog.set("AcroForm", acroform);
    }
    if let Some(metadata) = metadata {
        catalog.set("Metadata", metadata);
    }
    if let Some(outlines_id) = outlines {
        catalog.set("Outlines", outlines_id);
        if sources[0]
            .catalog()
            .ok()
            .and_then(|c| c.get(b"PageMode").ok())
            .and_then(|mode| mode.as_name().ok())
            == Some(b"UseOutlines")
        {
            catalog.set("PageMode", "UseOutlines");
        }
    }
    let catalog_id = out.add_object(catalog);
    out.trailer.set("Root", catalog_id);
    out
}

/// One bookmark on its way from the source document to the new one.
struct OutlineItem {
    title: Object,
    /// Destination array in output ids, first element the page.
    dest: Option<Vec<Object>>,
    colour: Option<Object>,
    flags: Option<Object>,
    open: bool,
    children: Vec<OutlineItem>,
}

/// Carry the bookmarks of `source` into `out`, keeping only those that point at
/// a page the new document contains.
///
/// A bookmark whose page did not come along is dropped, and if it had children
/// that did, they take its place rather than disappearing with it: a jump that
/// works is worth more than the indentation around it.
fn build_outlines(
    out: &mut Document,
    source: &Document,
    names: &BTreeMap<Vec<u8>, Object>,
    map: &BTreeMap<(usize, ObjectId), ObjectId>,
) -> Option<ObjectId> {
    let root = source
        .catalog()
        .ok()?
        .get(b"Outlines")
        .ok()?
        .as_reference()
        .ok()?;
    let first = source
        .get_dictionary(root)
        .ok()?
        .get(b"First")
        .ok()?
        .as_reference()
        .ok()?;
    let mut seen = BTreeSet::new();
    let items = outline_items(source, first, names, map, 0, &mut seen);
    if items.is_empty() {
        return None;
    }
    let root_id = out.add_object(dictionary! { "Type" => "Outlines" });
    let (span, visible) = write_outline_level(out, &items, root_id);
    let dict = out.get_dictionary_mut(root_id).ok()?;
    if let Some((first, last)) = span {
        dict.set("First", first);
        dict.set("Last", last);
    }
    dict.set("Count", visible);
    Some(root_id)
}

fn outline_items(
    source: &Document,
    first: ObjectId,
    names: &BTreeMap<Vec<u8>, Object>,
    map: &BTreeMap<(usize, ObjectId), ObjectId>,
    depth: usize,
    seen: &mut BTreeSet<ObjectId>,
) -> Vec<OutlineItem> {
    if depth > 32 {
        return Vec::new();
    }
    let mut items = Vec::new();
    let mut cursor = Some(first);
    while let Some(id) = cursor {
        // A `/Next` chain that loops back on itself would run forever.
        if !seen.insert(id) {
            break;
        }
        let Ok(dict) = source.get_dictionary(id) else {
            break;
        };
        cursor = dict
            .get(b"Next")
            .ok()
            .and_then(|next| next.as_reference().ok());
        let Some(title) = dict.get(b"Title").ok().cloned() else {
            continue;
        };
        let dest = dict
            .get(b"Dest")
            .ok()
            .or_else(|| dict.get(b"A").ok())
            .and_then(|target| outline_destination(target, source, names, map));
        let children = match dict.get(b"First").and_then(|f| f.as_reference()) {
            Ok(child) => outline_items(source, child, names, map, depth + 1, seen),
            Err(_) => Vec::new(),
        };
        if dest.is_none() {
            if children.is_empty() {
                continue;
            }
            // A section whose own page is gone still holds jumps worth keeping.
            items.extend(children);
            continue;
        }
        items.push(OutlineItem {
            title,
            dest,
            colour: dict.get(b"C").ok().cloned(),
            flags: dict.get(b"F").ok().cloned(),
            open: dict
                .get(b"Count")
                .ok()
                .and_then(|count| count.as_i64().ok())
                .map(|count| count >= 0)
                .unwrap_or(true),
            children,
        });
    }
    items
}

/// The destination of a bookmark as an array whose first entry points at a page
/// of the new document, or `None` when that page is not part of it.
fn outline_destination(
    target: &Object,
    source: &Document,
    names: &BTreeMap<Vec<u8>, Object>,
    map: &BTreeMap<(usize, ObjectId), ObjectId>,
) -> Option<Vec<Object>> {
    let resolved = resolve_destination(target, names, 0)?;
    let items = resolved.as_array().ok()?;
    let (page, rest) = items.split_first()?;
    let source_page = match page {
        Object::Reference(id) => *id,
        // Older producers write the page number rather than a reference.
        Object::Integer(number) => {
            let number = u32::try_from(*number).ok()?;
            *source.get_pages().get(&number)?
        }
        _ => return None,
    };
    let new_page = *map.get(&(0, source_page))?;
    let mut dest = Vec::with_capacity(items.len());
    dest.push(Object::Reference(new_page));
    dest.extend(rest.iter().cloned());
    Some(dest)
}

/// Write one level of the outline: the items are already reserved, so the chain
/// and each item's children can be filled in without holding a borrow across
/// the recursion. Returns the first and last id of the level, and how many
/// entries a reader shows when it is open.
fn write_outline_level(
    out: &mut Document,
    items: &[OutlineItem],
    parent: ObjectId,
) -> (Option<(ObjectId, ObjectId)>, i64) {
    let ids: Vec<ObjectId> = items
        .iter()
        .map(|_| out.add_object(Dictionary::new()))
        .collect();
    let mut spans: Vec<(Option<(ObjectId, ObjectId)>, i64)> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        spans.push(write_outline_level(out, &item.children, ids[index]));
    }
    let mut visible = 0i64;
    for (index, item) in items.iter().enumerate() {
        let (span, children_visible) = spans[index];
        let id = ids[index];
        let Ok(dict) = out.get_dictionary_mut(id) else {
            continue;
        };
        dict.set("Title", item.title.clone());
        dict.set("Parent", parent);
        if index > 0 {
            dict.set("Prev", ids[index - 1]);
        }
        if let Some(next) = ids.get(index + 1) {
            dict.set("Next", *next);
        }
        if let Some((first, last)) = span {
            dict.set("First", first);
            dict.set("Last", last);
        }
        if children_visible > 0 {
            dict.set(
                "Count",
                if item.open {
                    children_visible
                } else {
                    -children_visible
                },
            );
            visible += 1 + if item.open { children_visible } else { 0 };
        } else {
            visible += 1;
        }
        if let Some(dest) = &item.dest {
            dict.set("Dest", Object::Array(dest.clone()));
        }
        if let Some(colour) = &item.colour
            && colour.as_array().is_ok()
        {
            dict.set("C", colour.clone());
        }
        if let Some(flags) = &item.flags
            && flags.as_i64().is_ok()
        {
            dict.set("F", flags.clone());
        }
    }
    let span = ids
        .first()
        .zip(ids.last())
        .map(|(first, last)| (*first, *last));
    (span, visible)
}

// ---------------------------------------------------------------------------
// interactive forms
// ---------------------------------------------------------------------------

/// What one input brings to the merged form: its root fields, in source ids,
/// and the document level defaults that go with them.
struct SourceForm {
    base: u32,
    fields: Vec<ObjectId>,
    dr: Option<Object>,
    da: Option<Object>,
    q: Option<Object>,
    need_appearances: bool,
    sig_flags: i64,
    xfa: bool,
}

fn read_source_form(doc: &Document, base: u32, form: Object) -> Option<SourceForm> {
    let dict = match &form {
        Object::Reference(id) => doc.get_dictionary(*id).ok()?,
        Object::Dictionary(dict) => dict,
        _ => return None,
    };
    let fields = dict
        .get(b"Fields")
        .ok()
        .and_then(|fields| fields.as_array().ok())
        .map(|fields| {
            fields
                .iter()
                .filter_map(|field| field.as_reference().ok())
                .collect()
        })
        .unwrap_or_default();
    Some(SourceForm {
        base,
        fields,
        dr: dict.get(b"DR").ok().cloned(),
        da: dict.get(b"DA").ok().cloned(),
        q: dict.get(b"Q").ok().cloned(),
        need_appearances: dict
            .get(b"NeedAppearances")
            .ok()
            .and_then(|flag| flag.as_bool().ok())
            .unwrap_or(false),
        sig_flags: dict
            .get(b"SigFlags")
            .ok()
            .and_then(|flags| flags.as_i64().ok())
            .unwrap_or(0),
        // An XFA form is an XML document the viewer renders instead of the
        // pages; there is nothing sane to merge, so its form is left behind.
        xfa: dict.get(b"XFA").is_ok(),
    })
}

/// Carry the interactive form of every input into the merged document.
///
/// Every field object was copied with the rest, so all that has to be done is to
/// write one `/AcroForm` that owns them: one `/Fields` array, unique names among
/// the roots, and one default resource dictionary whose font names do not clash.
/// Two documents that both call a font `/Helv` are fine when it is the same
/// font; when it is not, the later one is renamed and the appearance strings
/// that named it are rewritten to match.
fn merge_forms(out: &mut Document, forms: &[SourceForm]) -> Option<ObjectId> {
    let usable: Vec<&SourceForm> = forms
        .iter()
        .filter(|form| !form.xfa && !form.fields.is_empty())
        .collect();
    if usable.is_empty() {
        return None;
    }

    let mut roots: Vec<(usize, ObjectId)> = Vec::new();
    for (index, form) in usable.iter().enumerate() {
        for id in &form.fields {
            let shifted = (id.0 + form.base, id.1);
            if !out.objects.contains_key(&shifted) {
                continue;
            }
            strip_actions(out, shifted, 0);
            roots.push((index, shifted));
        }
    }
    if roots.is_empty() {
        return None;
    }
    let ids: Vec<ObjectId> = roots.iter().map(|(_, id)| *id).collect();
    make_root_names_unique(out, &ids);
    let field_refs: Vec<Object> = ids.iter().map(|id| Object::Reference(*id)).collect();

    let mut groups: Vec<BTreeMap<Vec<u8>, ObjectId>> = Vec::new();
    for form in &usable {
        groups.push(font_map(out, form));
    }
    let merged = merge_fonts(out, &groups);
    for (index, renamed) in merged.renames.iter().enumerate() {
        if renamed.is_empty() {
            continue;
        }
        for (old, new) in renamed {
            for (owner, id) in &roots {
                if *owner != index {
                    continue;
                }
                rename_field_font(out, *id, old, new, 0);
            }
        }
    }

    let mut resources = dictionary! {};
    let mut fonts = Dictionary::new();
    for (name, id) in &merged.fonts {
        fonts.set(name.clone(), Object::Reference(*id));
    }
    if !fonts.is_empty() {
        resources.set("Font", fonts);
    }
    let mut acroform = dictionary! { "Fields" => field_refs };
    if !resources.is_empty() {
        let dr = out.add_object(resources);
        acroform.set("DR", dr);
    }
    let first = usable[0];
    if let Some(da) = &first.da {
        acroform.set("DA", da.clone());
    }
    if let Some(q) = &first.q {
        acroform.set("Q", q.clone());
    }
    let need_appearances = usable.iter().any(|form| form.need_appearances);
    if need_appearances {
        acroform.set("NeedAppearances", true);
    }
    let sig_flags = usable
        .iter()
        .map(|form| form.sig_flags)
        .reduce(|a, b| a | b)
        .unwrap_or(0);
    if sig_flags != 0 {
        acroform.set("SigFlags", sig_flags);
    }
    Some(out.add_object(acroform))
}

/// The `/DR /Font` entries of one input, as output ids.
fn font_map(out: &Document, form: &SourceForm) -> BTreeMap<Vec<u8>, ObjectId> {
    let mut fonts = BTreeMap::new();
    let Some(dr) = &form.dr else {
        return fonts;
    };
    let dict = match dr {
        Object::Reference(id) => out.get_dictionary((id.0 + form.base, id.1)).ok(),
        // A dictionary written inline in the catalogue was not copied with the
        // objects, so its fonts are looked up by the ids it holds plus the
        // input's offset.
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    };
    let Some(dict) = dict else {
        return fonts;
    };
    let shift = if matches!(dr, Object::Dictionary(_)) {
        form.base
    } else {
        0
    };
    let Some(font_dict) = dict
        .get(b"Font")
        .ok()
        .and_then(|fonts| fonts.as_dict().ok())
    else {
        return fonts;
    };
    for (name, value) in font_dict.iter() {
        if let Ok((number, generation)) = value.as_reference() {
            fonts.insert(name.to_vec(), (number + shift, generation));
        }
    }
    fonts
}

struct MergedFonts {
    fonts: BTreeMap<Vec<u8>, ObjectId>,
    /// Per input, the font names that had to change and what they became.
    renames: Vec<Vec<(Vec<u8>, Vec<u8>)>>,
}

/// Put the fonts of several inputs into one namespace.
///
/// A name that is already taken is kept when both inputs mean the same font;
/// otherwise the later one gets a new name and its caller rewrites the `/DA`
/// strings that used the old one.
fn merge_fonts(out: &Document, groups: &[BTreeMap<Vec<u8>, ObjectId>]) -> MergedFonts {
    let mut fonts: BTreeMap<Vec<u8>, ObjectId> = BTreeMap::new();
    let mut renames: Vec<Vec<(Vec<u8>, Vec<u8>)>> = Vec::new();
    for group in groups {
        let mut form_renames = Vec::new();
        for (name, id) in group {
            match fonts.get(name) {
                None => {
                    fonts.insert(name.clone(), *id);
                }
                Some(existing) if existing == id => {}
                Some(existing) => {
                    if same_font(out, *existing, *id) {
                        continue;
                    }
                    let new_name = unique_font_name(&fonts, name);
                    fonts.insert(new_name.clone(), *id);
                    form_renames.push((name.clone(), new_name));
                }
            }
        }
        renames.push(form_renames);
    }
    MergedFonts { fonts, renames }
}

fn same_font(out: &Document, a: ObjectId, b: ObjectId) -> bool {
    let name = |id| {
        out.get_dictionary(id)
            .ok()
            .and_then(|dict| {
                dict.get(b"BaseFont")
                    .ok()
                    .and_then(|font| font.as_name().ok())
                    .map(<[u8]>::to_vec)
            })
            .unwrap_or_default()
    };
    let kind = |id| {
        out.get_dictionary(id)
            .ok()
            .and_then(|dict| {
                dict.get(b"Subtype")
                    .ok()
                    .and_then(|kind| kind.as_name().ok())
                    .map(<[u8]>::to_vec)
            })
            .unwrap_or_default()
    };
    name(a) == name(b) && kind(a) == kind(b)
}

fn unique_font_name(fonts: &BTreeMap<Vec<u8>, ObjectId>, name: &[u8]) -> Vec<u8> {
    for suffix in 2.. {
        let candidate = [name, format!("_{suffix}").as_bytes()].concat();
        if !fonts.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Two forms in one document cannot both have a root field called "name": the
/// full name of a field is the path of partial names, and a duplicate makes
/// every reference to it ambiguous. The later one gets a suffix.
fn make_root_names_unique(out: &mut Document, roots: &[ObjectId]) {
    let mut taken: BTreeSet<Vec<u8>> = BTreeSet::new();
    for id in roots {
        let Ok(dict) = out.get_dictionary(*id) else {
            continue;
        };
        let Ok(title) = dict.get(b"T").and_then(|title| title.as_str()) else {
            continue;
        };
        let title = title.to_vec();
        let unique = unique_root_name(&taken, &title);
        if unique != title
            && let Ok(dict) = out.get_dictionary_mut(*id)
        {
            dict.set("T", Object::string_literal(unique.clone()));
        }
        taken.insert(unique);
    }
}

fn unique_root_name(taken: &BTreeSet<Vec<u8>>, name: &[u8]) -> Vec<u8> {
    if !taken.contains(name) {
        return name.to_vec();
    }
    for suffix in 2.. {
        let candidate = [name, format!("_{suffix}").as_bytes()].concat();
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!()
}

/// Drop the action dictionaries from a field and everything under it.
///
/// Those are where the JavaScript lives. A merge cannot keep them honest: a
/// calculation or a validation script names fields by their full name, and the
/// name it knows may have been made unique on the way in.
fn strip_actions(out: &mut Document, id: ObjectId, depth: usize) {
    if depth > 32 {
        return;
    }
    let kids: Vec<ObjectId> = match out.get_dictionary(id) {
        Ok(dict) => dict
            .get(b"Kids")
            .ok()
            .and_then(|kids| kids.as_array().ok())
            .map(|kids| {
                kids.iter()
                    .filter_map(|kid| kid.as_reference().ok())
                    .collect()
            })
            .unwrap_or_default(),
        Err(_) => return,
    };
    if let Ok(dict) = out.get_dictionary_mut(id) {
        dict.remove(b"AA");
        dict.remove(b"A");
    }
    for kid in kids {
        strip_actions(out, kid, depth + 1);
    }
}

/// Point a field's `/DA` at the font's new name.
fn rename_field_font(out: &mut Document, id: ObjectId, old: &[u8], new: &[u8], depth: usize) {
    if depth > 32 {
        return;
    }
    let (rewritten, kids): (Option<Vec<u8>>, Vec<ObjectId>) = match out.get_dictionary(id) {
        Ok(dict) => {
            let rewritten = dict
                .get(b"DA")
                .and_then(|da| da.as_str())
                .ok()
                .map(|da| rename_in_da(da, old, new));
            let kids = dict
                .get(b"Kids")
                .ok()
                .and_then(|kids| kids.as_array().ok())
                .map(|kids| {
                    kids.iter()
                        .filter_map(|kid| kid.as_reference().ok())
                        .collect()
                })
                .unwrap_or_default();
            (rewritten, kids)
        }
        Err(_) => return,
    };
    if let Some(da) = rewritten
        && let Ok(dict) = out.get_dictionary_mut(id)
    {
        dict.set("DA", Object::String(da, lopdf::StringFormat::Literal));
    }
    for kid in kids {
        rename_field_font(out, kid, old, new, depth + 1);
    }
}

/// Replace the font name in an appearance string like `/Helv 12 Tf 0 g`.
fn rename_in_da(da: &[u8], old: &[u8], new: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(da);
    let mut tokens: Vec<String> = text.split_whitespace().map(str::to_string).collect();
    let old = format!("/{}", String::from_utf8_lossy(old));
    let new = format!("/{}", String::from_utf8_lossy(new));
    let mut changed = false;
    for index in 0..tokens.len() {
        if tokens[index] == old
            && tokens
                .get(index + 1)
                .and_then(|size| size.parse::<f64>().ok())
                .is_some()
            && tokens.get(index + 2).map(String::as_str) == Some("Tf")
        {
            tokens[index] = new.clone();
            changed = true;
        }
    }
    if changed {
        tokens.join(" ").into_bytes()
    } else {
        da.to_vec()
    }
}

/// One node of a field tree on its way into an extracted document.
struct FieldNode {
    id: ObjectId,
    dict: Dictionary,
    widget: bool,
    children: Vec<FieldNode>,
}

fn field_root(source: &Document, widget: ObjectId) -> Option<ObjectId> {
    let mut id = widget;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(id) {
            return None;
        }
        let parent = source
            .get_dictionary(id)
            .ok()
            .and_then(|dict| dict.get(b"Parent").ok())
            .and_then(|parent| parent.as_reference().ok());
        match parent {
            Some(parent) => id = parent,
            None => return Some(id),
        }
    }
}

fn is_widget(dict: &Dictionary) -> bool {
    dict.get(b"Subtype")
        .ok()
        .and_then(|subtype| subtype.as_name().ok())
        == Some(b"Widget")
}

/// Keep the part of a field tree that leads to a widget which is in the output.
fn prune_field(
    source: &Document,
    id: ObjectId,
    kept: &BTreeSet<ObjectId>,
    depth: usize,
) -> Option<FieldNode> {
    if depth > 32 {
        return None;
    }
    let dict = source.get_dictionary(id).ok()?;
    if is_widget(dict) {
        if !kept.contains(&id) {
            return None;
        }
        return Some(FieldNode {
            id,
            dict: dict.clone(),
            widget: true,
            children: Vec::new(),
        });
    }
    let mut children = Vec::new();
    if let Ok(kids) = dict.get(b"Kids").and_then(|kids| kids.as_array()) {
        for kid in kids {
            let Ok(kid) = kid.as_reference() else {
                continue;
            };
            if let Some(node) = prune_field(source, kid, kept, depth + 1) {
                children.push(node);
            }
        }
    }
    // A field with no widget and no surviving child cannot be filled in.
    if children.is_empty() && dict.get(b"FT").is_err() {
        return None;
    }
    Some(FieldNode {
        id,
        dict: dict.clone(),
        widget: false,
        children,
    })
}

/// Write a pruned field tree into `out`, reusing the widget dictionaries the
/// page walk already copied and making new objects for the fields above them.
fn write_field_node(
    out: &mut Document,
    node: &FieldNode,
    parent: Option<ObjectId>,
    map: &BTreeMap<(usize, ObjectId), ObjectId>,
    source: usize,
) -> Option<ObjectId> {
    let id = if node.widget {
        *map.get(&(source, node.id))?
    } else {
        let mut dict = node.dict.clone();
        for key in [b"Kids".as_slice(), b"Parent", b"AA", b"A", b"P"] {
            dict.remove(key);
        }
        let mut object = Object::Dictionary(dict);
        strip_references(&mut object);
        let id = out.add_object(Dictionary::new());
        out.objects.insert(id, object);
        id
    };
    if let Some(parent) = parent
        && let Ok(dict) = out.get_dictionary_mut(id)
    {
        dict.set("Parent", parent);
    }
    let mut kids = Vec::new();
    for child in &node.children {
        if let Some(kid) = write_field_node(out, child, Some(id), map, source) {
            kids.push(Object::Reference(kid));
        }
    }
    if !kids.is_empty()
        && let Ok(dict) = out.get_dictionary_mut(id)
    {
        dict.set("Kids", kids);
    }
    Some(id)
}

/// Drop every entry whose value refers to another object.
fn strip_references(object: &mut Object) {
    let Object::Dictionary(dict) = object else {
        return;
    };
    let keys: Vec<Vec<u8>> = dict
        .iter()
        .filter(|(_, value)| has_reference(value))
        .map(|(key, _)| key.to_vec())
        .collect();
    for key in keys {
        dict.remove(&key);
    }
}

fn has_reference(object: &Object) -> bool {
    match object {
        Object::Reference(_) => true,
        Object::Array(items) => items.iter().any(has_reference),
        Object::Dictionary(dict) => dict.iter().any(|(_, value)| has_reference(value)),
        Object::Stream(stream) => stream.dict.iter().any(|(_, value)| has_reference(value)),
        _ => false,
    }
}

/// Build the interactive form for an extracted document.
///
/// The pages brought their widgets along, but a widget is only half of a field:
/// the tree above it lives in the catalogue being replaced. The tree is rebuilt
/// here, pruned to the widgets that are in the output, so a field whose page was
/// removed is not in the form and no widget is left without its field.
fn build_extracted_form(
    sources: &[Document],
    out: &mut Document,
    map: &BTreeMap<(usize, ObjectId), ObjectId>,
    keys: &[(usize, ObjectId)],
) -> Option<ObjectId> {
    // Widgets on the pages that were kept, in page order.
    let mut kept: BTreeMap<usize, BTreeSet<ObjectId>> = BTreeMap::new();
    let mut order: Vec<(usize, ObjectId)> = Vec::new();
    for (source, page) in keys {
        let Ok(dict) = sources[*source].get_dictionary(*page) else {
            continue;
        };
        let Ok(annots) = dict.get(b"Annots").and_then(|annots| annots.as_array()) else {
            continue;
        };
        for annot in annots {
            let Ok(id) = annot.as_reference() else {
                continue;
            };
            let Ok(annot) = sources[*source].get_dictionary(id) else {
                continue;
            };
            if !is_widget(annot) {
                continue;
            }
            let entry = kept.entry(*source).or_default();
            if entry.insert(id) {
                order.push((*source, id));
            }
        }
    }
    if order.is_empty() {
        return None;
    }

    // One tree per root field, even when several of its widgets were kept.
    let mut roots: Vec<(usize, ObjectId, Vec<ObjectId>)> = Vec::new();
    for (source, widget) in order {
        let Some(root) = field_root(&sources[source], widget) else {
            continue;
        };
        match roots
            .iter_mut()
            .find(|(owner, id, _)| *owner == source && *id == root)
        {
            Some((_, _, widgets)) => widgets.push(widget),
            None => roots.push((source, root, vec![widget])),
        }
    }
    if roots.is_empty() {
        return None;
    }

    let mut field_refs = Vec::new();
    let mut written: Vec<(usize, ObjectId)> = Vec::new();
    for (source, root, _) in &roots {
        let Some(node) = prune_field(&sources[*source], *root, &kept[source], 0) else {
            continue;
        };
        if let Some(id) = write_field_node(out, &node, None, map, *source) {
            field_refs.push(Object::Reference(id));
            written.push((*source, id));
        }
    }
    if field_refs.is_empty() {
        return None;
    }
    let root_ids: Vec<ObjectId> = written.iter().map(|(_, id)| *id).collect();
    make_root_names_unique(out, &root_ids);

    // One default resource dictionary per input, in the order they appear.
    let mut contributors: Vec<usize> = Vec::new();
    for (source, _) in &written {
        if !contributors.contains(source) {
            contributors.push(*source);
        }
    }
    let mut groups: Vec<BTreeMap<Vec<u8>, ObjectId>> = Vec::new();
    let mut defaults: Vec<Option<Dictionary>> = Vec::new();
    for source in &contributors {
        let form = acroform_dict(&sources[*source]);
        let mut fonts = BTreeMap::new();
        if let Some(table) = form
            .as_ref()
            .and_then(|form| form.get(b"DR").ok())
            .and_then(|dr| dr.as_dict().ok())
            .and_then(|dr| dr.get(b"Font").ok())
            .and_then(|fonts| fonts.as_dict().ok())
        {
            for (name, value) in table.iter() {
                let Ok(id) = value.as_reference() else {
                    continue;
                };
                let target = match map.get(&(*source, id)) {
                    Some(existing) => *existing,
                    // A font only the form used is not reachable from a page,
                    // so it was not copied: copy the dictionary itself when it
                    // does not point anywhere else.
                    None => {
                        let Ok(dict) = sources[*source].get_dictionary(id) else {
                            continue;
                        };
                        let mut object = Object::Dictionary(dict.clone());
                        strip_references(&mut object);
                        match object {
                            Object::Dictionary(dict) => out.add_object(dict),
                            _ => continue,
                        }
                    }
                };
                fonts.insert(name.to_vec(), target);
            }
        }
        groups.push(fonts);
        defaults.push(form);
    }

    let merged = merge_fonts(out, &groups);
    for (index, renamed) in merged.renames.iter().enumerate() {
        for (old, new) in renamed {
            for (owner, id) in &written {
                if *owner == contributors[index] {
                    rename_field_font(out, *id, old, new, 0);
                }
            }
        }
    }

    let mut fonts = Dictionary::new();
    for (name, id) in &merged.fonts {
        fonts.set(name.clone(), Object::Reference(*id));
    }
    let mut acroform = dictionary! { "Fields" => field_refs };
    if !fonts.is_empty() {
        let dr = out.add_object(dictionary! { "Font" => fonts });
        acroform.set("DR", dr);
    }
    if let Some(form) = defaults.first().cloned().flatten() {
        if let Ok(da) = form.get(b"DA") {
            acroform.set("DA", da.clone());
        }
        if let Ok(q) = form.get(b"Q") {
            acroform.set("Q", q.clone());
        }
        if form
            .get(b"NeedAppearances")
            .ok()
            .and_then(|flag| flag.as_bool().ok())
            .unwrap_or(false)
        {
            acroform.set("NeedAppearances", true);
        }
        let sig_flags = form
            .get(b"SigFlags")
            .ok()
            .and_then(|flags| flags.as_i64().ok())
            .unwrap_or(0);
        if sig_flags != 0 {
            acroform.set("SigFlags", sig_flags);
        }
    }
    Some(out.add_object(acroform))
}

/// Copy the XMP packet of `source` across. It is a stream of text with no
/// pointers of its own, so it can be moved as it stands; one that does hold a
/// reference is left behind rather than copied with a dead pointer.
fn copy_metadata(source: &Document, out: &mut Document) -> Option<ObjectId> {
    let id = source
        .catalog()
        .ok()?
        .get(b"Metadata")
        .ok()?
        .as_reference()
        .ok()?;
    let Ok(Object::Stream(stream)) = source.get_object(id) else {
        return None;
    };
    if stream.dict.iter().any(|(_, value)| has_reference(value)) {
        return None;
    }
    let mut stream = stream.clone();
    stream.dict.set("Length", stream.content.len() as i64);
    Some(out.add_object(stream))
}

fn acroform_dict(source: &Document) -> Option<Dictionary> {
    let form = source.catalog().ok()?.get(b"AcroForm").ok()?.clone();
    match form {
        Object::Reference(id) => source.get_dictionary(id).ok().cloned(),
        Object::Dictionary(dict) => Some(dict),
        _ => None,
    }
}

/// The document information dictionary of `source`, when it can be copied
/// without dragging anything behind it. A value that is a reference would point
/// at an object the extraction has no reason to keep, so a dictionary holding
/// one is left behind rather than copied with a broken pointer in it.
fn self_contained_info(source: &Document) -> Option<Dictionary> {
    let info = source.trailer.get(b"Info").ok()?;
    let dict = match info {
        Object::Reference(id) => source.get_dictionary(*id).ok()?,
        Object::Dictionary(dict) => dict,
        _ => return None,
    };
    let mut stack: Vec<&Object> = dict.iter().map(|(_, value)| value).collect();
    while let Some(object) = stack.pop() {
        match object {
            Object::Reference(_) => return None,
            Object::Array(items) => stack.extend(items.iter()),
            Object::Dictionary(inner) => stack.extend(inner.iter().map(|(_, value)| value)),
            _ => {}
        }
    }
    Some(dict.clone())
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
        let merged = merge(vec![a, b]).unwrap();
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
