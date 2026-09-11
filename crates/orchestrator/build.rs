use std::process::Command;

// Builds the UI so `include_str!("../ui/dist/index.html")` is always current. Needs `npm ci` in `ui/` once.
fn main() {
    for p in ["ui/src", "ui/index.html", "ui/vite.config.js", "ui/package-lock.json"] {
        println!("cargo:rerun-if-changed={p}");
    }
    let ok = Command::new("npm").args(["run", "build"]).current_dir("ui").status().map(|s| s.success()).unwrap_or(false);
    assert!(ok, "UI build failed; run `npm ci` in crates/orchestrator/ui first");
}
