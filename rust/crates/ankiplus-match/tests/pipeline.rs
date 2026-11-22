//! Port of `tests/mcp/test_match.py`: same fixture deck, same expectations.

use ankiplus_match::*;
use rusqlite::Connection;
use std::io::Write;
use std::path::{Path, PathBuf};

const AK: &str = "#AK_Step1_v12::#B&B::Anatomy::Upper_Limb::Brachial_Plexus";

pub fn make_collection(path: &Path) -> PathBuf {
    let db = Connection::open(path).unwrap();
    db.execute_batch(
        "create table col (decks text, models text); create table notes (id integer, mid integer, tags text, flds text);
         create table cards (id integer, nid integer, did integer, queue integer);",
    )
    .unwrap();
    db.execute(
        "insert into col values (?1, ?2)",
        [
            r#"{"1": {"name": "AnKing::Step1"}, "2": {"name": "Default"}}"#,
            r#"{"9": {"name": "AnKingOverhaul", "flds": [{"name": "Text", "ord": 0}, {"name": "Extra", "ord": 1}]}}"#,
        ],
    )
    .unwrap();
    let notes: Vec<(i64, [&str; 2], Vec<String>, Vec<(i64, i64)>)> = vec![
        (101, ["Thoracic outlet syndrome compresses the {{c1::lower trunk}} of the brachial plexus", "Pancoast tumour, cervical rib"],
         vec![AK.into(), "#AK_Step1_v12::#UWorld::Step::1234".into()], vec![(1001, -1)]),
        (102, ["Erb palsy injures the {{c1::upper trunk}} (C5-C6) of the brachial plexus", "<b>waiter tip</b>"], vec![AK.into()], vec![(1002, -1)]),
        (103, ["Klumpke palsy injures the lower trunk C8-T1 causing claw hand", ""], vec![AK.into()], vec![(1003, 0)]),
        (104, ["Sodium potassium ATPase pumps 3 sodium out and 2 potassium in", ""],
         vec!["#AK_Step1_v12::#B&B::Physiology::Membrane".into()], vec![(1004, -1), (1005, -1)]),
        (105, ["Myocardial infarction troponin rises within 4 hours", ""], vec!["#AK_Step1_v12::#B&B::Cardio".into()], vec![(1006, -1)]),
    ];
    for (nid, fields, tags, cards) in notes {
        db.execute("insert into notes values (?1, 9, ?2, ?3)", rusqlite::params![nid, tags.join(" "), fields.join("\x1f")]).unwrap();
        for (cid, queue) in cards {
            db.execute("insert into cards values (?1, ?2, 1, ?3)", rusqlite::params![cid, nid, queue]).unwrap();
        }
    }
    db.execute("insert into notes values (200, 9, '', 'Own card about sodium channels\x1f')", []).unwrap();
    db.execute("insert into cards values (2000, 200, 2, -1)", []).unwrap();
    path.to_path_buf()
}

/// Deterministic bag-of-words hashing: similar wording -> similar vectors.
struct FakeEmbedder;
impl FakeEmbedder {
    fn vec(text: &str) -> Vec<f32> {
        let mut v = vec![0f32; 32];
        for t in tokenize(text) {
            let h = t.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
            v[(h % 32) as usize] += 1.0;
        }
        v
    }
}
impl Embedder for FakeEmbedder {
    fn name(&self) -> String {
        "fake@32".into()
    }
    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Error> {
        Ok(texts.iter().map(|t| Self::vec(t)).collect())
    }
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, Error> {
        Ok(Self::vec(text))
    }
}

fn anking(dir: &Path) -> Vec<Note> {
    let deck = make_collection(&dir.join("collection.anki2"));
    load_deck(&deck, &LoadOptions { deck_prefix: Some("AnKing"), ..Default::default() }).unwrap()
}

#[test]
fn load_collection_and_package() {
    let tmp = tempfile::tempdir().unwrap();
    let deck = make_collection(&tmp.path().join("collection.anki2"));
    let notes = load_deck(&deck, &LoadOptions { deck_prefix: Some("AnKing"), ..Default::default() }).unwrap();
    assert_eq!(notes.iter().map(|n| n.note_id).collect::<Vec<_>>(), vec![101, 102, 103, 104, 105]);
    assert_eq!(notes[0].suspended_card_ids, vec![1001]);
    assert_eq!(notes[0].deck, "AnKing::Step1");
    assert!(notes[0].text.contains("lower trunk") && !notes[0].text.contains("{{"));
    assert!(notes[1].text.contains("waiter tip") && !notes[1].text.contains("<b>"));
    assert_eq!(load_deck(&deck, &LoadOptions::default()).unwrap().len(), 6);

    let apkg = tmp.path().join("AnKing.apkg");
    let mut z = zip::ZipWriter::new(std::fs::File::create(&apkg).unwrap());
    z.start_file("collection.anki21", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(&std::fs::read(&deck).unwrap()).unwrap();
    z.finish().unwrap();
    assert_eq!(load_deck(&apkg, &LoadOptions { deck_prefix: Some("AnKing"), ..Default::default() }).unwrap().len(), 5);

    let zstd_pkg = tmp.path().join("New.apkg");
    let mut z = zip::ZipWriter::new(std::fs::File::create(&zstd_pkg).unwrap());
    z.start_file("collection.anki21b", zip::write::SimpleFileOptions::default()).unwrap();
    z.write_all(&zstd::encode_all(&std::fs::read(&deck).unwrap()[..], 3).unwrap()).unwrap();
    z.finish().unwrap();
    assert_eq!(load_deck(&zstd_pkg, &LoadOptions::default()).unwrap().len(), 6);

    let json = tmp.path().join("notes.json");
    std::fs::write(&json, r#"[{"noteId": 7, "fields": {"Text": {"value": "{{c1::Na}} channel", "order": 0}}, "cards": [70], "tags": []}]"#).unwrap();
    let n = load_deck(&json, &LoadOptions::default()).unwrap();
    assert_eq!(n[0].text, "Na channel");
    assert_eq!(n[0].card_ids, vec![70]);
}

#[test]
fn search_and_concept_tags() {
    let tmp = tempfile::tempdir().unwrap();
    let index = DeckIndex::new(anking(tmp.path()), None, Default::default());
    let hits = index.search("sodium potassium pump moves 3 sodium out", 10, 50).unwrap();
    assert_eq!(index.notes[hits[0].doc].note_id, 104);
    assert!(hits[0].overlap > 0.6);
    let (tag, docs) = index.neighbours(0, 150);
    assert_eq!(tag.as_deref(), Some(AK));
    let mut ids: Vec<i64> = docs.iter().map(|d| index.notes[*d].note_id).collect();
    ids.sort();
    assert_eq!(ids, vec![102, 103]); // UWorld id tag ignored
}

#[test]
fn match_unsuspend_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let index = DeckIndex::new(anking(tmp.path()), None, Default::default());
    let chunks = chunks_from_text(
        "# Thoracic outlet\n- Thoracic outlet syndrome compresses the lower trunk of the brachial plexus\n\
         - Glomerular filtration rate depends on afferent arteriole tone\n",
    );
    let strict = Thresholds { covered: Some(0.6), related: Some(0.35), ..Default::default() };
    let report = match_chunks(&index, &chunks, &strict, &MatchOptions::default()).unwrap();
    let status: Vec<(&str, &str)> =
        report.chunks.iter().map(|r| (r.text.split(' ').next().unwrap(), r.status.as_str())).collect();
    assert_eq!(status, vec![("Thoracic", "covered"), ("Glomerular", "gap")]);
    assert_eq!(report.unsuspend.card_ids, vec![1001]);
    assert!(report.chunks[0].concepts.iter().any(|c| c.note_id == 102));
    assert_eq!(report.unsuspend.performed_anki_operations, 0);
    let with = match_chunks(&index, &chunks, &strict, &MatchOptions { unsuspend_concepts: true, ..Default::default() }).unwrap();
    assert_eq!(with.unsuspend.card_ids, vec![1001, 1002]); // 103 has no suspended card
    let prompt = build_prompt(&report, "L025.pdf", 24000);
    let gaps = prompt.split("## Gaps").nth(1).unwrap().split("##").next().unwrap();
    assert!(gaps.contains("Glomerular") && !gaps.contains("Thoracic"));
    assert!(prompt.contains("## Already covered"));
}

#[test]
fn embeddings_and_file_store() {
    let tmp = tempfile::tempdir().unwrap();
    let mut index = DeckIndex::new(anking(tmp.path()), Some(Box::new(FakeEmbedder)), Default::default());
    index.embed_all(256, |_, _| {}, None).unwrap();
    let q = "troponin rises after myocardial infarction";
    let hit = index.search(q, 10, 50).unwrap().remove(0);
    assert!(hit.cosine.is_some());
    assert_eq!(index.notes[hit.doc].note_id, 105);
    let store = FileIndexStore::new(tmp.path().join("idx"));
    let meta = store.save(&index).unwrap();
    assert_eq!(meta["embeddings"], true);
    assert_eq!(meta["notes"], 5);
    let loaded = store.load(Some(Box::new(FakeEmbedder))).unwrap();
    let v = loaded.vectors.as_ref().unwrap();
    assert_eq!((v.len(), v[0].len()), (5, 32));
    assert_eq!(loaded.search(q, 10, 50).unwrap()[0].doc, hit.doc);
    assert!(store.load(None).unwrap().vectors.is_none()); // no embedder -> BM25 only
}

#[test]
fn highlights_and_quote_check() {
    let chunks = chunks_from_highlights(&[
        Highlight { page: Some(3), text: "Erb palsy: upper trunk C5-C6\n".into(), note: Some("brachial".into()) },
        Highlight { page: None, text: "  ".into(), note: None },
    ]);
    assert_eq!(chunks.len(), 1);
    assert_eq!((chunks[0].id.as_str(), chunks[0].page), ("h1", 3));
    let cards: Vec<DraftCard> = serde_json::from_str(
        r#"[{"id": "a", "fields": {"Text": "Erb palsy hits the {{c1::upper trunk}}"}, "source_ref": [3]},
            {"id": "b", "fields": {"Text": "Erb palsy hits the {{c1::radial nerve}}"}, "source_ref": [3]}]"#,
    )
    .unwrap();
    let r = check_support(&cards, &chunks, 0.8);
    assert!(!r.cards[0].flag);
    assert!(r.cards[1].flag);
    assert_eq!(r.cards[1].answers_not_in_source, vec!["radial nerve"]);
}

#[test]
fn cache_key_matches_python_format() {
    let tmp = tempfile::tempdir().unwrap();
    let deck = make_collection(&tmp.path().join("c.anki2"));
    let key = pipeline::cache_key(&deck, None, pipeline::EmbeddingMode::Off).unwrap();
    assert!(key.starts_with('d') && key.len() == 17);
}

#[test]
fn threshold_defaults_follow_the_index_mode() {
    let t = Thresholds::default();
    assert_eq!((t.resolve(false).covered, t.resolve(false).related), (Some(0.5), Some(0.35)));
    assert_eq!((t.resolve(true).covered, t.resolve(true).related), (Some(0.7), Some(0.62)));
    assert_eq!(Thresholds { covered: Some(0.7), ..Default::default() }.resolve(true).covered, Some(0.7));
}

#[test]
fn synthetic_benchmark_recall() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/match");
    let notes = load_deck(&root.join("synthetic_anking.json"), &LoadOptions::default()).unwrap();
    let index = DeckIndex::new(notes, None, Default::default());
    let labels: Vec<Label> = serde_json::from_str(&std::fs::read_to_string(root.join("labels.json")).unwrap()).unwrap();
    assert!(evaluate(&index, &labels, 5, None).unwrap().recall_at_k.unwrap() >= 0.9);
}

fn note(id: i64, front: &str, text: &str, suspended: bool) -> Note {
    Note {
        note_id: id,
        card_ids: if suspended { vec![id + 10] } else { vec![] },
        suspended_card_ids: if suspended { vec![id + 10] } else { vec![] },
        text: text.into(),
        front: Some(front.into()),
        ..Default::default()
    }
}

#[test]
fn score_needs_both_directions_and_real_words() {
    let mut notes = vec![
        note(1, "3 2 = 9", "3 2 = 9", true),
        note(2, "The buccinator is the principal muscle of the cheek", "The buccinator is the principal muscle of the cheek", true),
        note(3, "Epistaxis most often arises from Kiesselbach plexus",
             "Epistaxis most often arises from Kiesselbach plexus\nsupplied by the labial artery of the facial artery and others", true),
    ];
    notes.extend((0..30).map(|i| note(100 + i, &format!("filler word{i}"), &format!("filler word{i} muscle artery"), false)));
    let index = DeckIndex::new(notes, None, Default::default());
    let score = |q: &str, id: i64| {
        index.search(q, 5, 50).unwrap().iter().find(|h| index.notes[h.doc].note_id == id).map_or(0.0, |h| h.score)
    };
    assert!(score("3 Histology 9 2", 1) < 0.35);
    assert!(score("Lateral wall: cheeks (buccinator muscle)", 2) >= 0.5);
    // the note's Extra mentions the words, but its fact is about something else
    assert!(score("Blood supply of the lips: labial arteries from the facial artery", 3) < 0.35);
}

#[test]
fn schema18_collection_with_unicase_indexes() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("collection.anki2");
    let db = Connection::open(&path).unwrap();
    db.create_collation("unicase", |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase())).unwrap();
    db.execute_batch(
        "create table col (decks text, models text);
         create table notes (id integer, mid integer, tags text, flds text);
         create table cards (id integer, nid integer, did integer, queue integer);
         create table decks (id integer, name text collate unicase);
         create table notetypes (id integer, name text collate unicase);
         create table fields (ntid integer, ord integer, name text collate unicase, primary key (ntid, ord)) without rowid;
         create unique index idx_fields_name_ntid on fields (name, ntid);
         create unique index idx_notetypes_name on notetypes (name);
         insert into col values ('', '');
         insert into decks values (1, 'AnKing');
         insert into notetypes values (9, 'AnKingOverhaul');
         insert into fields values (9, 0, 'Text'), (9, 1, 'Extra');
         insert into notes values (5, 9, '', 'The buccinator is the muscle of the cheek' || char(31) || 'extra');
         insert into cards values (50, 5, 1, -1);",
    )
    .unwrap();
    drop(db);
    let notes = load_deck(&path, &LoadOptions::default()).unwrap();
    assert!(notes[0].fields["Text"].starts_with("The buccinator"));
    assert_eq!(notes[0].front.as_deref(), Some("The buccinator is the muscle of the cheek"));
}

#[test]
fn anki_profiles() {
    use ankiplus_match::profiles::{anki_base_with, collection_for, list_profiles};
    use std::path::Path;
    let home = Path::new("/h");
    let none = |_: &str| None;
    assert_eq!(anki_base_with(&none, "darwin", home), home.join("Library/Application Support/Anki2"));
    assert_eq!(anki_base_with(&none, "linux", home), home.join(".local/share/Anki2"));
    assert_eq!(anki_base_with(&|k: &str| (k == "XDG_DATA_HOME").then(|| "/x".to_string()), "linux", home), Path::new("/x/Anki2"));
    assert_eq!(anki_base_with(&|k: &str| (k == "APPDATA").then(|| "/r".to_string()), "win32", home), Path::new("/r/Anki2"));
    assert_eq!(anki_base_with(&none, "win32", home), home.join("AppData/Roaming/Anki2"));
    assert_eq!(anki_base_with(&|k: &str| (k == "ANKI_BASE").then(|| "~/b".to_string()), "darwin", home), home.join("b"));

    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    assert!(collection_for(None, Some(base)).unwrap_err().to_string().contains("No Anki profiles"));
    for name in ["addons21", "Hamza", "User 1"] {
        std::fs::create_dir_all(base.join(name)).unwrap();
        std::fs::write(base.join(name).join("collection.anki2"), b"x").unwrap();
    }
    std::fs::create_dir_all(base.join("empty")).unwrap();
    let found: Vec<String> = list_profiles(Some(base)).into_iter().map(|p| p.profile).collect();
    assert_eq!(found, vec!["Hamza", "User 1"]);
    assert_eq!(collection_for(None, Some(base)).unwrap(), base.join("User 1").join("collection.anki2"));
    assert_eq!(collection_for(Some("Hamza"), Some(base)).unwrap(), base.join("Hamza").join("collection.anki2"));
    assert!(collection_for(Some("Nope"), Some(base)).unwrap_err().to_string().contains("Profiles: Hamza, User 1"));
    std::fs::remove_dir_all(base.join("User 1")).unwrap();
    assert_eq!(collection_for(None, Some(base)).unwrap(), base.join("Hamza").join("collection.anki2"));
}

/// Counts embedded texts, to check that unchanged notes keep their vectors.
struct CountingEmbedder(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl ankiplus_match::Embedder for CountingEmbedder {
    fn name(&self) -> String {
        FakeEmbedder.name()
    }
    fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, ankiplus_match::Error> {
        self.0.fetch_add(texts.len(), std::sync::atomic::Ordering::SeqCst);
        FakeEmbedder.embed_documents(texts)
    }
    fn embed_query(&self, text: &str) -> Result<Vec<f32>, ankiplus_match::Error> {
        FakeEmbedder.embed_query(text)
    }
}

#[test]
fn embed_all_reuses_unchanged_notes_and_written_cards_join_the_index() {
    use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};
    let tmp = tempfile::tempdir().unwrap();
    let notes = anking(tmp.path());
    let mut old = DeckIndex::new(notes.clone(), Some(Box::new(FakeEmbedder)), Default::default());
    assert_eq!(old.embed_all(256, |_, _| {}, None).unwrap(), ankiplus_match::EmbedCounts { embedded: 5, reused: 0 });

    let mut edited = notes.clone();
    edited[0].text.push_str(" edited");
    let calls = Arc::new(AtomicUsize::new(0));
    let mut fresh = DeckIndex::new(edited, Some(Box::new(CountingEmbedder(calls.clone()))), Default::default());
    let counts = fresh.embed_all(256, |_, _| {}, Some(&old)).unwrap();
    assert_eq!((counts.embedded, counts.reused, calls.load(Ordering::SeqCst)), (1, 4, 1));
    assert_eq!(fresh.vectors.as_ref().unwrap()[1], old.vectors.as_ref().unwrap()[1]);

    // Another model: nothing is reused.
    old.meta.insert("embedding_model".into(), "other".into());
    let mut again = DeckIndex::new(notes.clone(), Some(Box::new(FakeEmbedder)), Default::default());
    assert_eq!(again.embed_all(256, |_, _| {}, Some(&old)).unwrap().reused, 0);

    // Written cards: added (or replacing the same note id), only they are embedded.
    let written = vec![pipeline::WrittenCard {
        note_id: 9001,
        card_ids: vec![90011],
        deck: "ColdRecall".into(),
        tags: vec!["#ColdRecall::#Source::MED241::L025".into()],
        fields: vec!["The {{c1::sinoatrial}} node paces the heart".into(), "<b>Extra</b> line".into(), "ignored".into()],
    }];
    let deck = make_collection(&tmp.path().join("c.anki2"));
    let index_dir = tmp.path().join("indexes");
    let req = pipeline::IndexRequest {
        deck: &deck,
        deck_prefix: None,
        index_dir: &index_dir,
        embeddings: pipeline::EmbeddingMode::Off,
        backend: pipeline::Backend::File,
        rebuild: false,
    };
    let base = pipeline::open_index(&req, |_| {}).unwrap();
    let (grown, counts, added) = pipeline::index_written_cards(&req, &base, &written).unwrap();
    assert_eq!((added, counts.embedded, grown.notes.len()), (1, 0, base.notes.len() + 1));
    let note = grown.notes.last().unwrap();
    assert_eq!(note.text, "The sinoatrial node paces the heart\nExtra line");
    assert_eq!(note.front.as_deref(), Some("The sinoatrial node paces the heart"));
    assert_eq!(grown.search("sinoatrial node paces heart", 3, 50).unwrap()[0].doc, grown.notes.len() - 1);
    // The cached index has the new note, and the deck stamp is kept.
    let reopened = pipeline::open_index(&req, |_| {}).unwrap();
    assert_eq!(reopened.notes.len(), grown.notes.len());
}

#[test]
fn changed_deck_updates_the_same_cache_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let deck = make_collection(&tmp.path().join("c.anki2"));
    let index_dir = tmp.path().join("indexes");
    let req = pipeline::IndexRequest {
        deck: &deck,
        deck_prefix: None,
        index_dir: &index_dir,
        embeddings: pipeline::EmbeddingMode::Off,
        backend: pipeline::Backend::File,
        rebuild: false,
    };
    let mut msgs = vec![];
    pipeline::open_index(&req, |m| msgs.push(m.to_string())).unwrap();
    pipeline::open_index(&req, |m| msgs.push(m.to_string())).unwrap();
    assert_eq!(msgs.iter().filter(|m| m.starts_with("indexing")).count(), 1);
    let key = pipeline::cache_key(&deck, None, pipeline::EmbeddingMode::Off).unwrap();
    let meta: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(index_dir.join(&key).join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["deck_stamp"], pipeline::deck_stamp(&deck).unwrap());
    // Touch the deck: same folder, rebuilt.
    let f = std::fs::OpenOptions::new().append(true).open(&deck).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
    pipeline::open_index(&req, |m| msgs.push(m.to_string())).unwrap();
    assert_eq!(msgs.iter().filter(|m| m.starts_with("indexing")).count(), 2);
    assert_eq!(std::fs::read_dir(&index_dir).unwrap().count(), 1);
}

#[test]
fn wizard_builds_the_servers_named_index() {
    let tmp = tempfile::tempdir().unwrap();
    let deck = make_collection(&tmp.path().join("collection.anki2"));
    let decks = list_decks(&deck).unwrap();
    assert!(!decks.is_empty() && decks.iter().all(|d| d.notes > 0));
    let root = tmp.path().join("workspaces");
    let req = setup::NamedIndex {
        collection: &deck,
        root: &root,
        name: "anking",
        deck_prefix: None,
        embeddings: pipeline::EmbeddingMode::Off,
        profile: true,
    };
    let mut stages = vec![];
    let built = setup::build_named_index(&req, &mut |s, _, _| stages.push(s.to_string())).unwrap();
    assert!(built.notes > 0 && !built.embeddings);
    assert_eq!(stages.first().map(String::as_str), Some("reading deck"));
    let meta = setup::index_info(&root, "anking").unwrap();
    for key in ["name", "backend", "source_path", "profile", "deck_stamp", "embeddings_mode", "requested_backend", "format", "notes"] {
        assert!(meta.contains_key(key), "{key}");
    }
    assert_eq!(meta["profile"], true);
    if let Ok(dir) = std::env::var("ANKIPLUS_KEEP_WIZARD_INDEX") {
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(&deck, Path::new(&dir).join("collection.anki2")).unwrap();
        let req = setup::NamedIndex { collection: &Path::new(&dir).join("collection.anki2"), root: Path::new(&dir), ..req };
        setup::build_named_index(&req, &mut |_, _, _| {}).unwrap();
    }
}

fn tsv(lines: &[(i64, Vec<(i64, &str)>)]) -> String {
    let mut rows = vec!["level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext".to_string()];
    for (n, (top, words)) in lines.iter().enumerate() {
        for (w, (left, text)) in words.iter().enumerate() {
            rows.push(format!("5\t1\t1\t1\t{}\t{}\t{left}\t{top}\t{}\t30\t90\t{text}", n + 1, w + 1, text.len() * 20));
        }
    }
    rows.join("\n")
}

#[test]
fn ocr_layout_reads_columns_separately() {
    let t = tsv(&[
        (10, vec![(10, "Title"), (130, "across"), (280, "the"), (360, "whole"), (480, "slide"), (620, "width"), (760, "here")]),
        (100, vec![(10, "Predisposing"), (1000, "Candidiasis")]),
        (150, vec![(10, "Poor"), (110, "hygiene"), (1000, "White"), (1120, "patches")]),
        (200, vec![(10, "Immunocompromised"), (1000, "Dysphagia")]),
    ]);
    let got = ocr_layout(&t);
    assert_eq!(
        got.lines().collect::<Vec<_>>(),
        vec!["Title across the whole slide width here", "Predisposing", "Poor hygiene", "Immunocompromised", "Candidiasis", "White patches", "Dysphagia"]
    );
}

#[test]
fn headings_start_a_new_chunk() {
    let text = "Predisposing Factors:\n• Poor oral hygiene\n• Immunocompromised conditions\nTypes of Stomatitis\n• Ulcerative stomatitis";
    let chunks: Vec<String> = chunks_from_slides(&[Slide { n: 1, title: String::new(), text: text.into() }]).into_iter().map(|c| c.text).collect();
    assert_eq!(chunks, vec!["Predisposing Factors: Poor oral hygiene Immunocompromised conditions", "Types of Stomatitis Ulcerative stomatitis"]);
}

/// Python-vs-Rust check on generated inputs: ANKIPLUS_PARITY_CASES is a JSON list of
/// {"tsv": .., "layout": ..} and {"slides": [..], "chunks": [..]} written by the Python side.
#[test]
fn parity_cases_from_python() {
    let Ok(path) = std::env::var("ANKIPLUS_PARITY_CASES") else { return };
    let cases: Vec<serde_json::Value> = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (i, case) in cases.iter().enumerate() {
        if let Some(t) = case.get("quantities") {
            let got = serde_json::to_value(matcher::quantities(t.as_str().unwrap())).unwrap();
            assert_eq!(got, case["want"], "quantities case {i}: {t}");
        } else if let Some(t) = case.get("form") {
            assert_eq!(serde_json::to_value(quotes::form_problems(t.as_str().unwrap())).unwrap(), case["want"], "form case {i}");
        } else if let Some(t) = case.get("tsv") {
            assert_eq!(ocr_layout(t.as_str().unwrap()), case["layout"].as_str().unwrap(), "layout case {i}");
        } else {
            let slides: Vec<Slide> = serde_json::from_value(case["slides"].clone()).unwrap();
            let got: Vec<String> = chunks_from_slides(&slides).into_iter().map(|c| c.text).collect();
            let want: Vec<String> = serde_json::from_value(case["chunks"].clone()).unwrap();
            assert_eq!(got, want, "chunk case {i}");
        }
    }
}

#[test]
fn card_answers_map_cards_to_their_cloze() {
    let front = "CN {{c1::XII}} moves the tongue; a lesion deviates it {{c2::toward}} the lesion";
    let ords: indexmap::IndexMap<i64, i64> = [(11, 0), (12, 1), (13, 5)].into_iter().collect();
    let got = deck::card_answers(front, &ords);
    assert_eq!(got.into_iter().collect::<Vec<_>>(), vec![(11, "XII".to_string()), (12, "toward".to_string())]);
    assert!(deck::card_answers("Plain basic note", &ords).is_empty());
}

fn cloze_note(id: i64, front: &str, answers: &[(i64, &str)]) -> Note {
    let cards: Vec<i64> = answers.iter().map(|a| a.0).collect();
    Note {
        note_id: id,
        card_ids: cards.clone(),
        suspended_card_ids: cards,
        deck: "AnKing".into(),
        text: front.into(),
        front: Some(front.into()),
        card_answers: Some(answers.iter().map(|(c, a)| (*c, a.to_string())).collect()),
        ..Default::default()
    }
}

#[test]
fn only_the_sibling_that_tests_the_fact_and_number_conflicts() {
    let notes = vec![
        cloze_note(1, "CN XII lesion: the tongue deviates toward the side of the lesion", &[(11, "XII"), (12, "toward")]),
        cloze_note(2, "Jaundice is clinically visible when serum bilirubin exceeds 2.5 mg/dL", &[(21, "2.5 mg/dL")]),
        cloze_note(3, "Thoracic duct drains into the left subclavian vein", &[(31, "left subclavian")]),
    ];
    let mut index = DeckIndex::new(notes, None, Default::default());
    index.embed_all(64, |_, _| {}, None).unwrap();
    let chunk = |id: &str, page, text: &str| Chunk { id: id.into(), page, title: String::new(), text: text.into() };
    let chunks = vec![
        chunk("p1-1", 1, "Tongue deviates toward the side of the lesion"),
        chunk("p2-1", 2, "Jaundice is clinically visible when serum bilirubin exceeds 3 mg/dL"),
    ];
    let t = Thresholds { covered: Some(0.4), related: Some(0.3), ..Default::default() };
    let report = match_chunks(&index, &chunks, &t, &MatchOptions::default()).unwrap();
    let plan: std::collections::HashMap<i64, &UnsuspendNote> = report.unsuspend.notes.iter().map(|e| (e.note_id, e)).collect();
    assert_eq!(plan[&1].card_ids, vec![12]);
    assert!(plan[&1].answer_in_lecture);
    assert!(plan[&1].cards[0].question.starts_with("CN XII lesion: the tongue deviates [...]"));
    assert!(!plan.contains_key(&2) && !report.unsuspend.card_ids.contains(&21));
    let jaundice = &report.chunks[1];
    assert_eq!(jaundice.status, "conflict");
    assert_eq!(jaundice.conflicts[0].quantities[0].unit, "mg/dl");
    assert_eq!(report.summary.conflict, 1);
    let prompt = build_prompt(&report, "lecture", 24000);
    assert!(prompt.contains("## Conflicts") && prompt.contains("lecture [3.0] vs card [2.5] mg/dl"));
    assert!(report.unsuspend.instructions.contains("judge every note"));
}

#[test]
fn list_clozes_and_cut_sentences_are_flagged() {
    let p = quotes::form_problems("Causes: {{c1::a}}, {{c1::b}}, {{c1::c}} (and more");
    assert_eq!(p.len(), 2);
    assert!(p[0].starts_with("c1 hides 3 items"));
    assert!(p[1].starts_with("unpaired ()"));
    assert!(quotes::form_problems("{{c1::a}} and {{c2::b}}").is_empty());
}
