fn main() {
    // The MCP server release the app registers in Claude Desktop follows the repository's
    // package.json, so the app and the Python server cannot drift apart.
    let pkg = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../package.json");
    println!("cargo:rerun-if-changed={}", pkg.display());
    let version = std::fs::read_to_string(&pkg)
        .ok()
        .and_then(|s| s.split("\"version\"").nth(1).and_then(|r| r.split('"').nth(1)).map(String::from))
        .unwrap_or_else(|| "0.5.0".into());
    println!("cargo:rustc-env=ANKIPLUSMCP_VERSION={version}");
    tauri_build::build()
}
