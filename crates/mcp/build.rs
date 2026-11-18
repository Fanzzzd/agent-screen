use std::path::{Path, PathBuf};
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn watch(path: impl AsRef<Path>) {
    println!("cargo:rerun-if-changed={}", path.as_ref().display());
}

fn main() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    // Watch source directories too, so adding or removing an untracked source
    // file invalidates the identity. Never watch target/ or the enclosing root.
    for dir in ["apps", "crates", "tests", "vendor"] {
        watch(root.join(dir));
    }
    watch(root.join(".git")); // A worktree's gitfile, or an ordinary git directory.
    let repository = git(&root, &["rev-parse", "--show-toplevel"])
        .and_then(|p| PathBuf::from(p).canonicalize().ok())
        == root.canonicalize().ok();
    if !repository {
        for (key, value) in [
            ("REVISION", "unknown".to_string()),
            ("DIRTY", "unknown".to_string()),
            (
                "PROFILE",
                std::env::var("PROFILE").unwrap_or_else(|_| "unknown".into()),
            ),
            (
                "TARGET",
                std::env::var("TARGET").unwrap_or_else(|_| "unknown".into()),
            ),
        ] {
            println!("cargo:rustc-env=AGENTSCREEN_{key}={value}");
        }
        return;
    }
    if let Some(files) = git(&root, &["ls-files", "-z"]) {
        for file in files.split('\0').filter(|s| !s.is_empty()) {
            watch(root.join(file));
        }
    }
    for item in ["HEAD", "index", "refs", "packed-refs"] {
        if let Some(path) = git(&root, &["rev-parse", "--git-path", item]) {
            let path = PathBuf::from(path);
            watch(if path.is_absolute() {
                path
            } else {
                root.join(path)
            });
        }
    }
    let revision = git(&root, &["rev-parse", "--verify", "HEAD"])
        .filter(|r| r.len() == 40 && r.bytes().all(|c| c.is_ascii_hexdigit()))
        .unwrap_or_else(|| "unknown".into());
    let dirty = git(
        &root,
        &["status", "--porcelain", "--untracked-files=normal"],
    )
    .map(|s| if s.is_empty() { "false" } else { "true" })
    .unwrap_or("unknown");
    println!("cargo:rustc-env=AGENTSCREEN_REVISION={revision}");
    println!("cargo:rustc-env=AGENTSCREEN_DIRTY={dirty}");
    for (name, var) in [("PROFILE", "PROFILE"), ("TARGET", "TARGET")] {
        println!(
            "cargo:rustc-env=AGENTSCREEN_{name}={}",
            std::env::var(var).unwrap_or_else(|_| "unknown".into())
        );
    }
}
