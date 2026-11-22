//! Register the AnkiPlus MCP server in Claude Desktop without a terminal.
//!
//! Same result as `npx ankiplusmcp init --host claude-desktop --force`: the
//! `mcpServers.ankiplusmcp` entry in `claude_desktop_config.json` is added or replaced,
//! and an existing file is first copied to `<file>.ankiplusmcp-backup-<timestamp>`.
//! The entry runs the server through a private copy of `uv` (shipped inside the app),
//! which fetches Python and the server on first start: nothing to install by hand.

use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub const SERVER: &str = "ankiplusmcp";
pub const VERSION: &str = env!("ANKIPLUSMCP_VERSION");
const REPOSITORY: &str = "adeelahmad/ankiplusMCP";

/// The Python requirement uv runs. An archive URL, not `git+`, so the student's Mac
/// needs no git (or Xcode tools). `ANKIPLUSMCP_PY_SPEC` overrides it, as in the launcher.
pub fn py_spec() -> String {
    std::env::var("ANKIPLUSMCP_PY_SPEC").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| {
        format!("ankiplusmcp[all] @ https://github.com/{REPOSITORY}/archive/refs/heads/release-v{VERSION}.zip")
    })
}

fn home() -> PathBuf {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default()
}

/// Claude Desktop's config folder.
pub fn config_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support").join("Claude")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| home().join("AppData").join("Roaming")).join("Claude")
    } else {
        home().join(".config").join("Claude")
    }
}

pub fn config_file() -> PathBuf {
    config_dir().join("claude_desktop_config.json")
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// The uv shipped with the app (Tauri puts sidecars next to the main binary), else one on PATH.
pub fn bundled_uv() -> Option<PathBuf> {
    let here = std::env::current_exe().ok()?.parent()?.join(exe("uv"));
    if here.is_file() {
        return Some(here);
    }
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(exe("uv"))).find(|c| c.is_file()))
}

/// A stable copy of uv outside the app bundle, so the Claude entry keeps working when the
/// app runs from the disk image or is moved later.
pub fn installed_uv() -> PathBuf {
    ankiplus_match::setup::user_data_dir().join("bin").join(exe("uv"))
}

pub fn install_uv() -> Result<PathBuf, String> {
    let src = bundled_uv().ok_or("This build does not include uv; reinstall AnkiPlus from the release download.")?;
    let dst = installed_uv();
    if dst == src {
        return Ok(dst);
    }
    std::fs::create_dir_all(dst.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = dst.with_extension("new");
    std::fs::copy(&src, &tmp).map_err(|e| format!("could not copy uv: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &dst).map_err(|e| e.to_string())?;
    Ok(dst)
}

pub fn server_args(extra: &[String]) -> Vec<String> {
    let mut args = vec!["tool".into(), "run".into(), "--from".into(), py_spec(), SERVER.into()];
    args.extend(extra.iter().cloned());
    args
}

pub fn entry(uv: &Path, source_roots: &[PathBuf]) -> Value {
    let mut extra = vec!["serve".to_string()];
    for root in source_roots {
        extra.push("--source-root".into());
        extra.push(root.to_string_lossy().to_string());
    }
    json!({"command": uv.to_string_lossy(), "args": server_args(&extra)})
}

#[derive(Debug, Serialize)]
pub struct Registration {
    pub file: String,
    pub status: &'static str,
    pub backup: Option<String>,
}

fn timestamp() -> String {
    // Local time is not available without a time crate; UTC keeps backups ordered.
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Add or replace `mcpServers.ankiplusmcp` in `file`. A file that is not plain JSON is left
/// untouched (error), any other existing file is backed up first.
pub fn register(file: &Path, value: Value) -> Result<Registration, String> {
    let mut data: Map<String, Value> = match std::fs::read_to_string(file) {
        Ok(text) if text.trim().is_empty() => Map::new(),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(m)) => m,
            _ => {
                return Err(format!(
                    "{} is not plain JSON, so it was left untouched. Add this under \"mcpServers\" by hand:\n\"{SERVER}\": {}",
                    file.display(),
                    serde_json::to_string_pretty(&value).unwrap_or_default()
                ))
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(e.to_string()),
    };
    let servers = data.entry("mcpServers").or_insert_with(|| Value::Object(Map::new()));
    let Some(servers) = servers.as_object_mut() else {
        return Err(format!("\"mcpServers\" in {} is not an object; fix it by hand.", file.display()));
    };
    let status = match servers.get(SERVER) {
        Some(old) if *old == value => return Ok(Registration { file: file.display().to_string(), status: "unchanged", backup: None }),
        Some(_) => "replaced",
        None => "added",
    };
    servers.insert(SERVER.into(), value);
    let mut backup = None;
    if file.is_file() {
        let b = PathBuf::from(format!("{}.{SERVER}-backup-{}", file.display(), timestamp()));
        std::fs::copy(file, &b).map_err(|e| e.to_string())?;
        backup = Some(b.display().to_string());
    } else {
        std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    }
    std::fs::write(file, serde_json::to_string_pretty(&Value::Object(data)).map_err(|e| e.to_string())? + "\n")
        .map_err(|e| e.to_string())?;
    Ok(Registration { file: file.display().to_string(), status, backup })
}

pub fn registered(file: &Path) -> Option<Value> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    v.get("mcpServers")?.get(SERVER).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_adds_replaces_and_backs_up() {
        let tmp = std::env::temp_dir().join(format!("ankiplus-claude-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let file = tmp.join("Claude").join("claude_desktop_config.json");
        let a = entry(Path::new("/x/uv"), &[]);
        let r = register(&file, a.clone()).unwrap();
        assert_eq!((r.status, r.backup.is_none()), ("added", true));
        assert_eq!(register(&file, a.clone()).unwrap().status, "unchanged");
        // Other servers and settings survive a replacement.
        let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        v["mcpServers"]["other"] = json!({"command": "o"});
        v["theme"] = json!("dark");
        std::fs::write(&file, v.to_string()).unwrap();
        let b = entry(Path::new("/y/uv"), &[PathBuf::from("/docs")]);
        let r = register(&file, b.clone()).unwrap();
        assert_eq!(r.status, "replaced");
        assert!(Path::new(r.backup.as_ref().unwrap()).is_file());
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!((v["mcpServers"]["other"]["command"].as_str(), v["theme"].as_str()), (Some("o"), Some("dark")));
        assert_eq!(registered(&file), Some(b.clone()));
        let args: Vec<&str> = b["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
        assert_eq!(&args[..2], &["tool", "run"]);
        assert!(args[3].contains(&format!("release-v{VERSION}.zip")));
        assert_eq!(&args[4..], &["ankiplusmcp", "serve", "--source-root", "/docs"]);
        // Not JSON: untouched.
        std::fs::write(&file, "{ // comment\n}").unwrap();
        assert!(register(&file, a).unwrap_err().contains("left untouched"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ // comment\n}");
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(timestamp().len(), 14);
    }
}
