//! PDF text extraction through docling.rs (text layer only: no layout or OCR models).
//!
//! Text items are grouped by their provenance page, so page numbers stay true to the
//! PDF even when a slide has no text. Pages whose text layer is empty or garbled
//! (a broken font encoding turns 'The' into '7KH') are OCR'd with the system
//! `tesseract`, rendered by poppler's `pdftoppm` at the same 216 dpi the Python side
//! uses. Without those programs such a page stays empty.

use crate::chunks::{chunks_from_pages, looks_garbled, Chunk};
use crate::Error;
use docling::{DocumentConverter, SourceDocument};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// `(page number, text)` for every page, page 1 first.
pub fn pdf_pages(path: &Path) -> Result<Vec<(i64, String)>, Error> {
    let source = SourceDocument::from_file(path).map_err(|e| Error::Source(format!("{}: {e}", path.display())))?;
    let result = DocumentConverter::new()
        .text_layer_only(true)
        .convert(source)
        .map_err(|e| Error::Source(format!("Could not read {}: {e}", path.display())))?;
    let doc = result.document.export_to_json_value();
    let mut pages: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    if let Some(Value::Object(map)) = doc.get("pages") {
        for key in map.keys() {
            if let Ok(n) = key.parse() {
                pages.entry(n).or_default();
            }
        }
    }
    for item in doc.get("texts").and_then(Value::as_array).into_iter().flatten() {
        let text = item.get("text").and_then(Value::as_str).unwrap_or("").trim();
        let page = item
            .get("prov")
            .and_then(Value::as_array)
            .and_then(|p| p.first())
            .and_then(|p| p.get("page_no"))
            .and_then(Value::as_i64);
        if let (false, Some(page)) = (text.is_empty(), page) {
            pages.entry(page).or_default().push(text.to_string());
        }
    }
    Ok(pages.into_iter().map(|(n, lines)| (n, lines.join("\n"))).collect())
}

/// OCR one page (1-based); empty when poppler or tesseract is missing or fails.
pub fn ocr_page(path: &Path, page: i64) -> String {
    use std::process::Command;
    let Ok(tmp) = tempfile::tempdir() else { return String::new() };
    let prefix = tmp.path().join("page");
    let rendered = Command::new("pdftoppm")
        .args(["-r", "216", "-png", "-singlefile", "-f", &page.to_string(), "-l", &page.to_string()])
        .arg(path)
        .arg(&prefix)
        .output();
    if !rendered.is_ok_and(|o| o.status.success()) {
        return String::new();
    }
    match Command::new("tesseract").arg(prefix.with_extension("png")).args(["stdout", "--psm", "3", "-l", "eng"]).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

/// Pages with a usable text layer, OCR-ing those whose text is garbled or empty.
pub fn pdf_pages_ocr(path: &Path, only: Option<&[i64]>, ocr: bool) -> Result<Vec<(i64, String)>, Error> {
    Ok(pdf_pages(path)?
        .into_iter()
        .filter(|(n, _)| only.is_none_or(|o| o.contains(n)))
        .map(|(n, text)| {
            if ocr && looks_garbled(&text) {
                (n, ocr_page(path, n))
            } else {
                (n, text)
            }
        })
        .collect())
}

/// Chunks from a PDF, optionally limited to some pages.
pub fn chunks_from_pdf(path: &Path, only: Option<&[i64]>) -> Result<Vec<Chunk>, Error> {
    Ok(chunks_from_pages(&pdf_pages_ocr(path, only, true)?))
}
