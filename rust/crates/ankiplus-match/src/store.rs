//! Index persistence behind one trait, so matching code never cares where an index lives.
//!
//! `FileIndexStore` writes the same folder layout as the Python `FileIndexStore`
//! (`notes.jsonl.gz`, `meta.json`, `embeddings.npy` as float16), so an index built
//! by either implementation loads in the other.

use crate::deck::Note;
use crate::index::{DeckIndex, Embedder};
use crate::Error;
use flate2::{read::GzDecoder, write::GzEncoder, Compression};
use serde_json::{Map, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};

pub const FORMAT: i64 = 2;

pub trait IndexStore {
    fn save(&self, index: &DeckIndex) -> Result<Map<String, Value>, Error>;
    fn load(&self, embedder: Option<Box<dyn Embedder + Send + Sync>>) -> Result<DeckIndex, Error>;
}

pub struct FileIndexStore {
    pub folder: PathBuf,
}

impl FileIndexStore {
    pub fn new(folder: impl Into<PathBuf>) -> Self {
        Self { folder: folder.into() }
    }
}

pub(crate) fn index_meta(index: &DeckIndex, extra: &[(&str, Value)]) -> Map<String, Value> {
    let mut meta = index.meta.clone();
    meta.insert("format".into(), FORMAT.into());
    for (k, v) in extra {
        meta.insert((*k).into(), v.clone());
    }
    meta.insert("notes".into(), index.notes.len().into());
    meta.insert(
        "suspended_cards".into(),
        index.notes.iter().map(|n| n.suspended_card_ids.len()).sum::<usize>().into(),
    );
    meta.insert("embeddings".into(), index.vectors.is_some().into());
    if index.vectors.is_none() {
        meta.shift_remove("embedding_model");
    }
    meta
}

pub(crate) fn check_format(meta: &Map<String, Value>) -> Result<(), Error> {
    if meta.get("format").and_then(Value::as_i64) != Some(FORMAT) {
        return Err(Error::Index("Index format changed; rebuild it with index_deck.".into()));
    }
    Ok(())
}

pub(crate) fn same_model(meta: &Map<String, Value>, embedder: &Option<Box<dyn Embedder + Send + Sync>>) -> bool {
    match embedder {
        Some(e) => meta.get("embedding_model").and_then(Value::as_str) == Some(e.name().as_str()),
        None => false,
    }
}

impl IndexStore for FileIndexStore {
    fn save(&self, index: &DeckIndex) -> Result<Map<String, Value>, Error> {
        std::fs::create_dir_all(&self.folder)?;
        let mut gz = GzEncoder::new(std::fs::File::create(self.folder.join("notes.jsonl.gz"))?, Compression::default());
        for note in &index.notes {
            serde_json::to_writer(&mut gz, note)?;
            gz.write_all(b"\n")?;
        }
        gz.finish()?;
        let vectors = self.folder.join("embeddings.npy");
        match &index.vectors {
            Some(v) => write_npy_f16(&vectors, v)?,
            None => {
                let _ = std::fs::remove_file(&vectors);
            }
        }
        let meta = index_meta(index, &[]);
        std::fs::write(self.folder.join("meta.json"), serde_json::to_string_pretty(&meta)? + "\n")?;
        Ok(meta)
    }

    fn load(&self, embedder: Option<Box<dyn Embedder + Send + Sync>>) -> Result<DeckIndex, Error> {
        let meta: Map<String, Value> = serde_json::from_str(&std::fs::read_to_string(self.folder.join("meta.json"))?)?;
        check_format(&meta)?;
        let reader = BufReader::new(GzDecoder::new(std::fs::File::open(self.folder.join("notes.jsonl.gz"))?));
        let mut notes: Vec<Note> = Vec::new();
        for line in reader.lines() {
            let line = line?;
            if !line.trim().is_empty() {
                notes.push(serde_json::from_str(&line)?);
            }
        }
        let vectors = self.folder.join("embeddings.npy");
        let reuse = vectors.is_file() && same_model(&meta, &embedder);
        let mut index = DeckIndex::new(notes, embedder, meta);
        if reuse {
            index.vectors = Some(read_npy(&vectors)?);
        }
        Ok(index)
    }
}

/// Write a 2-D little-endian float16 `.npy` (format 1.0), as `np.save(x.astype(np.float16))` does.
pub fn write_npy_f16(path: &Path, rows: &[Vec<f32>]) -> Result<(), Error> {
    let cols = rows.first().map_or(0, Vec::len);
    let mut header = format!("{{'descr': '<f2', 'fortran_order': False, 'shape': ({}, {}), }}", rows.len(), cols);
    let total = 10 + header.len() + 1;
    header.push_str(&" ".repeat((64 - total % 64) % 64));
    header.push('\n');
    let mut out = Vec::with_capacity(10 + header.len() + rows.len() * cols * 2);
    out.extend_from_slice(b"\x93NUMPY\x01\x00");
    out.extend_from_slice(&(header.len() as u16).to_le_bytes());
    out.extend_from_slice(header.as_bytes());
    for row in rows {
        for &x in row {
            out.extend_from_slice(&half::f16::from_f32(x).to_le_bytes());
        }
    }
    std::fs::write(path, out)?;
    Ok(())
}

/// Read a 2-D C-order `.npy` of `<f2` or `<f4` into f32 rows.
pub fn read_npy(path: &Path) -> Result<Vec<Vec<f32>>, Error> {
    let mut data = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut data)?;
    let bad = || Error::Index(format!("Unsupported .npy file: {}", path.display()));
    if data.len() < 10 || &data[..6] != b"\x93NUMPY" {
        return Err(bad());
    }
    let (hlen, start) = if data[6] == 1 {
        (u16::from_le_bytes([data[8], data[9]]) as usize, 10)
    } else {
        (u32::from_le_bytes([data[8], data[9], data[10], data[11]]) as usize, 12)
    };
    let header = std::str::from_utf8(&data[start..start + hlen]).map_err(|_| bad())?;
    if header.contains("'fortran_order': True") {
        return Err(bad());
    }
    let width = if header.contains("'<f2'") {
        2
    } else if header.contains("'<f4'") {
        4
    } else {
        return Err(bad());
    };
    let shape = header.split("'shape': (").nth(1).and_then(|s| s.split(')').next()).ok_or_else(bad)?;
    let dims: Vec<usize> = shape.split(',').filter_map(|d| d.trim().parse().ok()).collect();
    let (n, d) = match dims[..] {
        [n, d] => (n, d),
        [0] => (0, 0),
        _ => return Err(bad()),
    };
    let body = &data[start + hlen..];
    if body.len() < n * d * width {
        return Err(bad());
    }
    let value = |i: usize| -> f32 {
        let b = &body[i * width..(i + 1) * width];
        if width == 2 {
            half::f16::from_le_bytes([b[0], b[1]]).to_f32()
        } else {
            f32::from_le_bytes([b[0], b[1], b[2], b[3]])
        }
    };
    Ok((0..n).map(|r| (0..d).map(|c| value(r * d + c)).collect()).collect())
}
