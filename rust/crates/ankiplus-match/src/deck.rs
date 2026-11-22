//! Read notes and their cards from a deck export or a read-only collection copy.
//!
//! Supported inputs (same as the Python `load_deck`):
//! - `.apkg` / `.colpkg` zips (`collection.anki21b` is zstd-compressed,
//!   `collection.anki21` / `collection.anki2` are plain SQLite),
//! - `.anki2` / `.anki21` / `.db` / `.sqlite` collections (copied first, never opened for writing),
//! - `.json` / `.jsonl` note lists, e.g. saved from the Anki MCP `notes_info` output.

use crate::text::clean_text;
use crate::Error;
use indexmap::IndexMap;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

const SUSPENDED: i64 = -1;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub note_id: i64,
    #[serde(default)]
    pub card_ids: Vec<i64>,
    #[serde(default)]
    pub suspended_card_ids: Vec<i64>,
    #[serde(default)]
    pub deck: String,
    #[serde(default, skip_serializing)]
    pub notetype: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing)]
    pub fields: IndexMap<String, String>,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct LoadOptions<'a> {
    /// Keep notes with a card in this deck or a subdeck of it.
    pub deck_prefix: Option<&'a str>,
    /// Fields used for the searchable text (default: the first two).
    pub fields: Option<&'a [String]>,
    pub suspended_only: bool,
}

pub fn load_deck(path: &Path, opts: &LoadOptions) -> Result<Vec<Note>, Error> {
    if !path.is_file() {
        return Err(Error::Deck(format!("Deck file not found: {}", path.display())));
    }
    let suffix = path
        .extension()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let notes = match suffix.as_str() {
        "json" | "jsonl" => from_json(path)?,
        "apkg" | "colpkg" => from_package(path)?,
        "anki2" | "anki21" | "db" | "sqlite" => from_sqlite_copy(path)?,
        _ => {
            return Err(Error::Deck(
                "Use an .apkg, .colpkg, .anki2 collection, or a .json/.jsonl note list.".into(),
            ))
        }
    };
    let mut out = Vec::new();
    for mut note in notes {
        if let Some(prefix) = opts.deck_prefix {
            if !under(&note.deck, prefix) {
                continue;
            }
        }
        if opts.suspended_only && note.suspended_card_ids.is_empty() {
            continue;
        }
        let chosen: Vec<&String> = match opts.fields {
            Some(names) => names.iter().filter_map(|n| note.fields.get(n)).collect(),
            None => note.fields.values().take(2).collect(),
        };
        note.text = chosen
            .into_iter()
            .map(|v| clean_text(v))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if !note.text.is_empty() {
            out.push(note);
        }
    }
    Ok(out)
}

fn under(deck: &str, prefix: &str) -> bool {
    deck == prefix || deck.starts_with(&format!("{prefix}::"))
}

fn as_i64(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn ids(v: Option<&Value>) -> Vec<i64> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(as_i64).collect())
        .unwrap_or_default()
}

fn str_field<'a>(row: &'a Value, a: &str, b: &str) -> String {
    row.get(a)
        .or_else(|| row.get(b))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn from_json(path: &Path) -> Result<Vec<Note>, Error> {
    let text = std::fs::read_to_string(path)?;
    let rows: Vec<Value> = if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("jsonl")) {
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()?
    } else {
        match serde_json::from_str::<Value>(&text)? {
            Value::Array(a) => a,
            Value::Object(o) => o
                .get("notes")
                .filter(|v| v.as_array().is_some_and(|a| !a.is_empty()))
                .or_else(|| o.get("result"))
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            _ => vec![],
        }
    };
    let mut notes = Vec::new();
    for row in rows {
        let mut fields = IndexMap::new();
        match row.get("fields") {
            Some(Value::Object(map)) if !map.is_empty() && map.values().all(Value::is_object) => {
                // Anki MCP / AnkiConnect shape: {"Text": {"value": "...", "order": 0}}
                let mut items: Vec<(&String, &Value)> = map.iter().collect();
                items.sort_by_key(|(_, v)| v.get("order").and_then(Value::as_i64).unwrap_or(0));
                for (k, v) in items {
                    fields.insert(k.clone(), v.get("value").and_then(Value::as_str).unwrap_or("").to_string());
                }
            }
            Some(Value::Object(map)) => {
                for (k, v) in map {
                    fields.insert(k.clone(), v.as_str().unwrap_or("").to_string());
                }
            }
            Some(Value::Array(list)) => {
                for (i, v) in list.iter().enumerate() {
                    fields.insert(format!("Field{}", i + 1), v.as_str().unwrap_or("").to_string());
                }
            }
            _ => {}
        }
        if fields.is_empty() {
            if let Some(t) = row.get("text").and_then(Value::as_str) {
                fields.insert("Text".into(), t.to_string());
            }
        }
        let note_id = row
            .get("note_id")
            .or_else(|| row.get("noteId"))
            .and_then(as_i64)
            .ok_or_else(|| Error::Deck("Every JSON note needs note_id (or noteId).".into()))?;
        let cards = if row.get("card_ids").is_some() { ids(row.get("card_ids")) } else { ids(row.get("cards")) };
        notes.push(Note {
            note_id,
            card_ids: cards,
            suspended_card_ids: ids(row.get("suspended_card_ids")),
            deck: str_field(&row, "deck", "deckName"),
            notetype: str_field(&row, "notetype", "modelName"),
            tags: row
                .get("tags")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                .unwrap_or_default(),
            fields,
            text: String::new(),
        });
    }
    Ok(notes)
}

fn from_package(path: &Path) -> Result<Vec<Note>, Error> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?).map_err(|e| Error::Deck(e.to_string()))?;
    let tmp = tempfile::tempdir()?;
    let target = tmp.path().join("collection.db");
    let read = |zip: &mut zip::ZipArchive<std::fs::File>, name: &str| -> Option<Vec<u8>> {
        let mut entry = zip.by_name(name).ok()?;
        let mut buf = Vec::new();
        entry.read_to_end(&mut buf).ok()?;
        Some(buf)
    };
    if let Some(data) = read(&mut zip, "collection.anki21b") {
        let raw = zstd::decode_all(&data[..]).map_err(|e| Error::Deck(format!("Could not decompress collection.anki21b: {e}")))?;
        std::fs::write(&target, raw)?;
    } else if let Some(data) = read(&mut zip, "collection.anki21").or_else(|| read(&mut zip, "collection.anki2")) {
        std::fs::write(&target, data)?;
    } else {
        return Err(Error::Deck("Package has no collection file.".into()));
    }
    from_sqlite(&target)
}

fn from_sqlite_copy(path: &Path) -> Result<Vec<Note>, Error> {
    // Anki may hold the live file open; read a private copy, including any WAL.
    let tmp = tempfile::tempdir()?;
    let target = tmp.path().join("collection.db");
    std::fs::copy(path, &target)?;
    let wal = path.with_file_name(format!("{}-wal", path.file_name().unwrap().to_string_lossy()));
    if wal.is_file() {
        std::fs::copy(&wal, tmp.path().join("collection.db-wal"))?;
    }
    from_sqlite(&target)
}

struct CardInfo {
    card_ids: Vec<i64>,
    suspended: Vec<i64>,
    deck: String,
}

fn from_sqlite(path: &Path) -> Result<Vec<Note>, Error> {
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let tables: Vec<String> = db
        .prepare("select name from sqlite_master where type='table'")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let has = |t: &str| tables.iter().any(|x| x == t);
    if !has("notes") || !has("cards") {
        return Err(Error::Deck("Not an Anki collection (no notes/cards tables).".into()));
    }
    let (decks, models, field_names) = names(&db, &has)?;
    let mut cards: HashMap<i64, CardInfo> = HashMap::new();
    {
        let mut stmt = db.prepare("select id, nid, did, queue from cards")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let (cid, nid, did, queue): (i64, i64, i64, i64) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            let entry = cards.entry(nid).or_insert_with(|| CardInfo {
                card_ids: vec![],
                suspended: vec![],
                deck: decks.get(&did).cloned().unwrap_or_default(),
            });
            entry.card_ids.push(cid);
            if queue == SUSPENDED {
                entry.suspended.push(cid);
            }
        }
    }
    let mut notes = Vec::new();
    let mut stmt = db.prepare("select id, mid, tags, flds from notes")?;
    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let (nid, mid, tags, flds): (i64, i64, String, String) = (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
        let values: Vec<&str> = flds.split('\x1f').collect();
        let generated: Vec<String>;
        let names: &[String] = match field_names.get(&mid) {
            Some(n) if !n.is_empty() => n,
            _ => {
                generated = (1..=values.len()).map(|i| format!("Field{i}")).collect();
                &generated
            }
        };
        let fields: IndexMap<String, String> =
            names.iter().cloned().zip(values.iter().map(|v| v.to_string())).collect();
        let info = cards.remove(&nid);
        let (card_ids, suspended, deck) = match info {
            Some(i) => (i.card_ids, i.suspended, i.deck),
            None => (vec![], vec![], String::new()),
        };
        notes.push(Note {
            note_id: nid,
            card_ids,
            suspended_card_ids: suspended,
            deck,
            notetype: models.get(&mid).cloned().unwrap_or_default(),
            tags: tags.split_whitespace().map(String::from).collect(),
            fields,
            text: String::new(),
        });
    }
    Ok(notes)
}

type Names = (HashMap<i64, String>, HashMap<i64, String>, HashMap<i64, Vec<String>>);

fn names(db: &Connection, has: &dyn Fn(&str) -> bool) -> Result<Names, Error> {
    let mut decks = HashMap::new();
    let mut models = HashMap::new();
    let mut fields: HashMap<i64, Vec<String>> = HashMap::new();
    if has("decks") {
        // schema 18+
        let mut stmt = db.prepare("select id, name from decks")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (id, name) = row?;
            decks.insert(id, name.replace('\x1f', "::"));
        }
    }
    if has("notetypes") {
        let mut stmt = db.prepare("select id, name from notetypes")?;
        for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (id, name) = row?;
            models.insert(id, name);
        }
        if has("fields") {
            let mut stmt = db.prepare("select ntid, name from fields order by ntid, ord")?;
            for row in stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
                let (id, name) = row?;
                fields.entry(id).or_default().push(name);
            }
        }
    }
    if decks.is_empty() || models.is_empty() {
        let row: Option<(Option<String>, Option<String>)> = db
            .query_row("select decks, models from col", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok();
        if let Some((d, m)) = row {
            if decks.is_empty() {
                if let Some(Value::Object(map)) = d.filter(|s| !s.is_empty()).and_then(|s| serde_json::from_str(&s).ok()) {
                    for (k, v) in map {
                        if let Ok(id) = k.parse() {
                            decks.insert(id, v.get("name").and_then(Value::as_str).unwrap_or("").to_string());
                        }
                    }
                }
            }
            if models.is_empty() {
                if let Some(Value::Object(map)) = m.filter(|s| !s.is_empty()).and_then(|s| serde_json::from_str(&s).ok()) {
                    for (k, v) in map {
                        let Ok(id) = k.parse() else { continue };
                        models.insert(id, v.get("name").and_then(Value::as_str).unwrap_or("").to_string());
                        let mut flds: Vec<&Value> = v.get("flds").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
                        flds.sort_by_key(|f| f.get("ord").and_then(Value::as_i64).unwrap_or(0));
                        fields.insert(id, flds.iter().map(|f| f.get("name").and_then(Value::as_str).unwrap_or("").to_string()).collect());
                    }
                }
            }
        }
    }
    Ok((decks, models, fields))
}
