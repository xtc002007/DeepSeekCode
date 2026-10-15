use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=../../scripts/git-hooks/pre-commit");
    // P1-10: opt-in only. Type crates must have zero build side effects:
    // read-only checkouts, cross-compiles, and `cargo package` previously
    // failed or mutated global git config on every `cargo build`.
    // Set `CODE_PLUS_PLUS_INSTALL_HOOKS=1` to enable the one-time hooks setup.
    if std::env::var("CODE_PLUS_PLUS_INSTALL_HOOKS").as_deref() == Ok("1") {
        ensure_git_hooks();
    }
}

fn ensure_git_hooks() {
    // CARGO_MANIFEST_DIR is always set by Cargo; fail loudly with context
    // instead of a bare `unwrap()` so a broken invocation is diagnosable.
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("build.rs: CARGO_MANIFEST_DIR not set"),
    );
    let Some(root) = find_repo_root(&manifest_dir) else {
        return;
    };

    let hooks_path = "scripts/git-hooks";
    if !root.join(hooks_path).join("pre-commit").is_file() {
        return;
    }

    let current = Command::new("git")
        .args(["config", "--get", "core.hooksPath"])
        .current_dir(&root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string());

    if current.as_deref() == Some(hooks_path) {
        return;
    }

    let configured = Command::new("git")
        .args(["config", "core.hooksPath", hooks_path])
        .current_dir(&root)
        .status()
        .map(|status| status.success())
        .unwrap_or(false);

    if configured {
        println!(
            "cargo:warning=Git hooks enabled (core.hooksPath={hooks_path}). `git commit` will scan staged secrets automatically."
        );
    }
}

fn find_repo_root(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        if ancestor.join(".git").exists() && ancestor.join("scripts/git-hooks").exists() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}
