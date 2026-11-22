// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--self-test") {
        // `--self-test <deck> <report.json>`: the release build has no console on Windows.
        let report = args.get(3).map(std::path::PathBuf::from);
        std::process::exit(ankiplus_desktop_lib::self_test(args.get(2).map(std::path::Path::new), report.as_deref()));
    }
    ankiplus_desktop_lib::run()
}
