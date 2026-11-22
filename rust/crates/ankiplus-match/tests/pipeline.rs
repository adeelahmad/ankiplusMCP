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
    let report = match_chunks(&index, &chunks, &Thresholds::default(), &MatchOptions::default()).unwrap();
    let status: Vec<(&str, &str)> =
        report.chunks.iter().map(|r| (r.text.split(' ').next().unwrap(), r.status.as_str())).collect();
    assert_eq!(status, vec![("Thoracic", "covered"), ("Glomerular", "gap")]);
    assert_eq!(report.unsuspend.card_ids, vec![1001]);
    assert!(report.chunks[0].concepts.iter().any(|c| c.note_id == 102));
    assert_eq!(report.unsuspend.performed_anki_operations, 0);
    let with = match_chunks(&index, &chunks, &Thresholds::default(), &MatchOptions { unsuspend_concepts: true, ..Default::default() }).unwrap();
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
    index.embed_all(256, |_, _| {}).unwrap();
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
