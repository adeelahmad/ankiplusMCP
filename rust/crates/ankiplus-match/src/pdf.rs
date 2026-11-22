//! PDF text extraction through docling.rs (text layer only: no layout or OCR models).
//!
//! Text items are grouped by their provenance page, so page numbers stay true to the
//! PDF even when a slide has no text (an image-only slide is just an empty page).

use crate::chunks::{chunks_from_pages, Chunk};
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

/// Chunks from a PDF, optionally limited to some pages.
pub fn chunks_from_pdf(path: &Path, only: Option<&[i64]>) -> Result<Vec<Chunk>, Error> {
    let pages: Vec<(i64, String)> = pdf_pages(path)?
        .into_iter()
        .filter(|(n, _)| only.is_none_or(|o| o.contains(n)))
        .collect();
    Ok(chunks_from_pages(&pages))
}
