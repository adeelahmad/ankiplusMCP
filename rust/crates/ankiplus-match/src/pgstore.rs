//! Embedded Postgres + pgvector index storage.
//!
//! The server runs from a private data folder (`postgresql_embedded`), listens on
//! localhost only, and gets pgvector from prebuilt packages on first use. One schema
//! per index with the same tables as the Python `PostgresIndexStore`: `notes` rows
//! carry text, tags, card IDs and the embedding (HNSW cosine index). Matching still
//! scores in process, so results are identical to the file store.

use crate::deck::Note;
use crate::index::{DeckIndex, Embedder};
use crate::store::{check_format, index_meta, same_model, IndexStore};
use crate::Error;
use postgresql_embedded::blocking::PostgreSQL;
use postgresql_embedded::{Settings, VersionReq};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const DATABASE: &str = "ankiplus";

static SERVERS: OnceLock<Mutex<HashMap<PathBuf, (PostgreSQL, String)>>> = OnceLock::new();

fn pg_err(e: impl std::fmt::Display) -> Error {
    Error::Index(format!("Postgres: {e}"))
}

fn password(dir: &Path) -> Result<String, Error> {
    let file = dir.join(".password");
    if let Ok(p) = std::fs::read_to_string(&file) {
        return Ok(p.trim().to_string());
    }
    let mut bytes = [0u8; 18];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes))
        .unwrap_or_else(|_| {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
            bytes[..16].copy_from_slice(&t.to_le_bytes());
        });
    let p: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(&file, &p)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(p)
}

/// The prebuilt x86-64 pgvector packages are compiled for AVX-512 (EVEX instructions even
/// in the default distance functions), so their backend dies with SIGILL on CPUs without
/// it (Intel Core 12th-14th gen, AMD before Zen 4, most Intel Macs). Same guard as the
/// Python `pgstore.available()`.
pub fn cpu_supports_bundled_pgvector() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("avx512f")
            && std::arch::is_x86_feature_detected!("avx512vl")
            && std::arch::is_x86_feature_detected!("avx512bw")
            && std::arch::is_x86_feature_detected!("avx512dq")
            && std::arch::is_x86_feature_detected!("avx512vpopcntdq")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

/// One embedded server per data folder, started on first use; returns its database URL.
///
/// `ANKIPLUS_PG_URL` points at an existing server with pgvector instead (for example
/// the one the Python side runs, or a machine where GitHub downloads are blocked).
pub fn server_url(data_dir: &Path) -> Result<String, Error> {
    if let Ok(url) = std::env::var("ANKIPLUS_PG_URL") {
        if !url.is_empty() {
            return Ok(url);
        }
    }
    if !cpu_supports_bundled_pgvector() {
        return Err(Error::Index(
            "Embedded Postgres is off on this CPU: the bundled pgvector needs AVX-512 and would crash. \
             Use the file store, or set ANKIPLUS_PG_URL to a server with pgvector."
                .into(),
        ));
    }
    std::fs::create_dir_all(data_dir)?;
    let key = std::fs::canonicalize(data_dir)?;
    let servers = SERVERS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut servers = servers.lock().map_err(pg_err)?;
    if let Some((_, url)) = servers.get(&key) {
        return Ok(url.clone());
    }
    // Postgres 16: portalcorp/pgvector_compiled ships prebuilt pgvector only for pg16
    // (macOS arm64, Windows and Linux x86-64); pg17 fails with "asset not found".
    let settings = Settings {
        version: VersionReq::parse("=16").map_err(pg_err)?,
        installation_dir: key.join("install"),
        data_dir: key.join("data"),
        password_file: key.join(".pgpass"),
        password: password(&key)?,
        host: "localhost".into(),
        temporary: false,
        ..Settings::default()
    };
    let mut pg = PostgreSQL::new(settings);
    pg.setup().map_err(pg_err)?;
    let installed = postgresql_extensions::blocking::get_installed_extensions(pg.settings()).unwrap_or_default();
    if !installed.iter().any(|e| e.name().contains("pgvector")) {
        postgresql_extensions::blocking::install(
            pg.settings(),
            "portal-corp",
            "pgvector_compiled",
            &postgresql_extensions::VersionReq::STAR,
        )
        .map_err(|e| pg_err(format!("could not install pgvector ({e})")))?;
    }
    pg.start().map_err(pg_err)?;
    if !pg.database_exists(DATABASE).map_err(pg_err)? {
        pg.create_database(DATABASE).map_err(pg_err)?;
    }
    let url = pg.settings().url(DATABASE);
    servers.insert(key, (pg, url.clone()));
    Ok(url)
}

/// Stop every server this process started.
pub fn shutdown() {
    if let Some(servers) = SERVERS.get() {
        if let Ok(mut servers) = servers.lock() {
            for (_, (pg, _)) in servers.drain() {
                let _ = pg.stop();
            }
        }
    }
}

pub struct PostgresIndexStore {
    data_dir: PathBuf,
    schema: String,
}

impl PostgresIndexStore {
    pub fn new(data_dir: impl Into<PathBuf>, name: &str) -> Result<Self, Error> {
        let clean = name.replace('-', "_");
        if clean.is_empty() || clean.len() > 60 || !clean.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::Index("Invalid index name.".into()));
        }
        Ok(Self { data_dir: data_dir.into(), schema: format!("idx_{}", clean.to_lowercase()) })
    }

    fn connect(&self) -> Result<postgres::Client, Error> {
        postgres::Client::connect(&server_url(&self.data_dir)?, postgres::NoTls).map_err(pg_err)
    }
}

fn vector_literal(v: &[f32]) -> String {
    let parts: Vec<String> = v.iter().map(|x| format!("{x}")).collect();
    format!("[{}]", parts.join(","))
}

impl IndexStore for PostgresIndexStore {
    fn save(&self, index: &DeckIndex) -> Result<Map<String, Value>, Error> {
        let dim = index.vectors.as_ref().and_then(|v| v.first()).map(Vec::len);
        let s = &self.schema;
        let mut db = self.connect()?;
        let mut tx = db.transaction().map_err(pg_err)?;
        let vec_col = dim.map_or(String::new(), |d| format!(", embedding vector({d})"));
        tx.batch_execute(&format!(
            "create extension if not exists vector;
             drop schema if exists {s} cascade;
             create schema {s};
             create table {s}.notes (doc integer primary key, note_id bigint not null,
                card_ids bigint[] not null, suspended_card_ids bigint[] not null, deck text, tags text[],
                text text not null, front text, card_answers jsonb, tsv tsvector{vec_col});
             create table {s}.meta (key text primary key, value jsonb);"
        ))
        .map_err(pg_err)?;
        let insert = if dim.is_some() {
            format!("insert into {s}.notes (doc, note_id, card_ids, suspended_card_ids, deck, tags, text, front, card_answers, embedding) values ($1, $2, $3, $4, $5, $6, $7, $8, $9::text::jsonb, $10::text::vector)")
        } else {
            format!("insert into {s}.notes (doc, note_id, card_ids, suspended_card_ids, deck, tags, text, front, card_answers) values ($1, $2, $3, $4, $5, $6, $7, $8, $9::text::jsonb)")
        };
        let stmt = tx.prepare(&insert).map_err(pg_err)?;
        for (i, n) in index.notes.iter().enumerate() {
            let doc = i as i32;
            let answers = serde_json::to_string(&n.card_answers.clone().unwrap_or_default())?;
            match &index.vectors {
                Some(v) => {
                    let lit = vector_literal(&v[i]);
                    tx.execute(&stmt, &[&doc, &n.note_id, &n.card_ids, &n.suspended_card_ids, &n.deck, &n.tags, &n.text, &n.front, &answers, &lit])
                }
                None => tx.execute(&stmt, &[&doc, &n.note_id, &n.card_ids, &n.suspended_card_ids, &n.deck, &n.tags, &n.text, &n.front, &answers]),
            }
            .map_err(pg_err)?;
        }
        tx.batch_execute(&format!(
            "update {s}.notes set tsv = to_tsvector('english', text);
             create index on {s}.notes using gin (tsv);"
        ))
        .map_err(pg_err)?;
        if dim.is_some() {
            tx.batch_execute(&format!("create index on {s}.notes using hnsw (embedding vector_cosine_ops);"))
                .map_err(pg_err)?;
        }
        let meta = index_meta(index, &[("backend", Value::String("postgres".into())), ("card_answers", Value::Bool(true))]);
        let json = serde_json::to_string(&meta)?;
        tx.execute(&format!("insert into {s}.meta values ('meta', $1::text::jsonb)"), &[&json]).map_err(pg_err)?;
        tx.commit().map_err(pg_err)?;
        Ok(meta)
    }

    fn load(&self, embedder: Option<Box<dyn Embedder + Send + Sync>>) -> Result<DeckIndex, Error> {
        let s = &self.schema;
        let mut db = self.connect()?;
        let row = db.query_one(&format!("select value::text from {s}.meta where key='meta'"), &[]).map_err(pg_err)?;
        let meta: Map<String, Value> = serde_json::from_str(row.get::<_, &str>(0))?;
        check_format(&meta)?;
        let with_vec = meta.get("embeddings").and_then(Value::as_bool).unwrap_or(false);
        let vec_sel = if with_vec { ", embedding::text" } else { "" };
        // Older indexes lack the column.
        let has_answers = meta.get("card_answers").and_then(Value::as_bool).unwrap_or(false);
        let answers = if has_answers { "card_answers::text" } else { "null::text" };
        let rows = db
            .query(&format!("select note_id, card_ids, suspended_card_ids, deck, tags, text, front, {answers}{vec_sel} from {s}.notes order by doc"), &[])
            .map_err(pg_err)?;
        let notes: Vec<Note> = rows
            .iter()
            .map(|r| Note {
                note_id: r.get(0),
                card_ids: r.get(1),
                suspended_card_ids: r.get(2),
                deck: r.get::<_, Option<String>>(3).unwrap_or_default(),
                tags: r.get::<_, Option<Vec<String>>>(4).unwrap_or_default(),
                text: r.get(5),
                front: r.get(6),
                card_answers: Some(
                    r.get::<_, Option<&str>>(7).and_then(|t| serde_json::from_str(t).ok()).unwrap_or_default(),
                ),
                ..Default::default()
            })
            .collect();
        let reuse = with_vec && same_model(&meta, &embedder);
        let mut index = DeckIndex::new(notes, embedder, meta);
        if reuse {
            let vectors = rows
                .iter()
                .map(|r| serde_json::from_str::<Vec<f32>>(r.get::<_, &str>(8)))
                .collect::<Result<Vec<_>, _>>()?;
            index.vectors = Some(vectors);
        }
        Ok(index)
    }
}
