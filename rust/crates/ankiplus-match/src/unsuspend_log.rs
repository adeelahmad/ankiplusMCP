//! The unsuspend log shared with the Python MCP server (`unsuspended.jsonl`).
//!
//! One JSON row per report from the host: `{time, match_id, source, status, card_ids, detail, ...}`.
//! CARD ids in rows with `status == "success"` are never proposed again.

use crate::Error;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::Write;
use std::path::Path;

/// CARD ids already unsuspended successfully; empty when the log does not exist.
pub fn already_unsuspended(log: &Path) -> HashSet<i64> {
    let Ok(text) = std::fs::read_to_string(log) else { return HashSet::new() };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|row| row.get("status").and_then(Value::as_str) == Some("success"))
        .flat_map(|row| row.get("card_ids").and_then(Value::as_array).cloned().unwrap_or_default())
        .filter_map(|c| c.as_i64())
        .collect()
}

/// Append what the host reports it unsuspended (`status`: success, partial, failed, ...).
pub fn record(log: &Path, card_ids: &[i64], status: &str, match_id: Option<&str>, source: Option<&str>, detail: &str) -> Result<Value, Error> {
    let mut ids: Vec<i64> = card_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let row = json!({
        "time": format_utc(secs), "workspace": Value::Null, "match_id": match_id, "source": source,
        "status": status, "card_ids": ids, "detail": detail.chars().take(500).collect::<String>(),
        "evidence_origin": "caller_supplied",
    });
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut out = std::fs::OpenOptions::new().create(true).append(true).open(log)?;
    out.write_all((serde_json::to_string(&row)? + "\n").as_bytes())?;
    Ok(row)
}

/// `YYYY-MM-DDTHH:MM:SS+00:00`, like Python's `isoformat(timespec='seconds')` in UTC.
fn format_utc(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}+00:00", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_and_reads_back_successes_only() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("idx/unsuspended.jsonl");
        record(&log, &[3, 1, 1], "success", Some("m1"), None, "").unwrap();
        record(&log, &[9], "failed", None, None, "Anki closed").unwrap();
        assert_eq!(already_unsuspended(&log), HashSet::from([1, 3]));
        assert_eq!(format_utc(0), "1970-01-01T00:00:00+00:00");
        assert_eq!(format_utc(1_795_000_000), "2026-11-18T11:06:40+00:00");
    }
}
