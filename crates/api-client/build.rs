// FACTORY_VERSION: the crate version, plus `+<short commit>` when the tree is not exactly at the tag v<version> or has
// local changes. No git (a build from a tarball) means the bare version.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    if let Some(dir) = git(&["rev-parse", "--git-dir"]) {
        for f in ["HEAD", "index", "packed-refs", "refs/tags"] {
            println!("cargo:rerun-if-changed={dir}/{f}");
        }
    }
    let tagged = git(&["describe", "--tags", "--exact-match", "HEAD"]).is_some_and(|t| t == format!("v{version}"));
    let clean = git(&["status", "--porcelain"]).is_some_and(|s| s.is_empty());
    let version = match git(&["rev-parse", "--short", "HEAD"]) {
        Some(sha) if !(tagged && clean) => format!("{version}+{sha}"),
        _ => version.to_string(),
    };
    println!("cargo:rustc-env=FACTORY_VERSION={version}");
}
