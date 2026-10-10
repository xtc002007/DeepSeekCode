//! Full Git operation surface for the Cursor-parity Git panel (P0-P5).
//!
//! Every function is a thin, blocking wrapper around the `git` CLI so the
//! desktop can call it from `spawn_blocking`. Product logging follows the
//! existing `git` module convention: STORAGE target with `run_id`.
//!
//! No business logic / parsing lives here beyond exit-code handling —
//! porcelain parsing lives in `deepseekcode-desktop::logic` (unit-testable
//! without spawning git).

use deepseekcode_types::{DeepSeekCodeError, Result};
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::process::Command;

fn git_cmd(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo);
    crate::process::hide_console(&mut cmd);
    // Avoid paging / colour / interactive prompts in a GUI context.
    cmd.env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0");
    cmd
}

fn run(repo: &Path, args: &[&str]) -> Result<String> {
    let mut cmd = git_cmd(repo);
    cmd.args(args);
    let output = cmd.output().map_err(|e| {
        tracing::warn!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            repo = %repo.display(),
            args = ?args,
            error = %e,
            "git spawn failed"
        );
        DeepSeekCodeError::Other(format!("git command failed: {e}"))
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        tracing::warn!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            repo = %repo.display(),
            args = ?args,
            detail = %detail,
            "git command failed"
        );
        return Err(DeepSeekCodeError::Other(if detail.is_empty() {
            format!("git {} failed", args.join(" "))
        } else {
            detail
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_os(repo: &Path, args: &[OsString]) -> Result<String> {
    let mut cmd = git_cmd(repo);
    cmd.args(args);
    let log_args = args
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    let output = cmd
        .output()
        .map_err(|e| DeepSeekCodeError::Other(format!("git command failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        tracing::warn!(
            target: deepseekcode_telemetry::STORAGE,
            run_id = %deepseekcode_telemetry::current_run_id(),
            repo = %repo.display(),
            args = %log_args,
            detail = %detail,
            "git command failed"
        );
        return Err(DeepSeekCodeError::Other(if detail.is_empty() {
            format!("git {log_args} failed")
        } else {
            detail
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn paths_as_os(paths: &[String]) -> Vec<OsString> {
    paths.iter().map(|p| OsStr::new(p).to_os_string()).collect()
}

// ---------------------------------------------------------------------------
// Status / diff (read-only, P0)
// ---------------------------------------------------------------------------

/// `git status --porcelain=v2 -uall -b` raw output (branch header + entries).
pub fn status_porcelain_v2(repo: &Path) -> Result<String> {
    run(
        repo,
        &["status", "--porcelain=v2", "-uall", "-b", "--show-stash"],
    )
    .or_else(|_| run(repo, &["status", "--porcelain=v2", "-uall", "-b"]))
    .or_else(|_| run(repo, &["status", "--porcelain", "-uall", "-b"]))
}

/// `git diff --shortstat` summary line ("20 files changed, ...").
pub fn diff_shortstat(repo: &Path) -> Option<String> {
    run(repo, &["diff", "--shortstat"])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Names in the index (`git diff --cached --name-only`). Ground truth for
/// "is there anything staged" — never trust cached UI state for this, it can
/// lag behind external git operations and produce cryptic hook failures
/// (e.g. lint-staged "could not find any staged files").
pub fn staged_names(repo: &Path) -> Result<Vec<String>> {
    run(repo, &["diff", "--cached", "--name-only"]).map(|s| {
        s.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect()
    })
}

/// True when the working tree has any uncommitted change (staged or not).
pub fn has_uncommitted_changes(repo: &Path) -> Result<bool> {
    run(repo, &["status", "--porcelain", "-uall"]).map(|s| {
        s.lines()
            .map(str::trim)
            .any(|l| !l.is_empty())
    })
}

/// Unified diff for one path vs HEAD (falls back to unstaged-only diff).
pub fn diff_for_path(repo: &Path, path: &str) -> Result<String> {
    let out = run_os(
        repo,
        &[
            OsString::from("diff"),
            OsString::from("HEAD"),
            OsString::from("--"),
            OsStr::new(path).to_os_string(),
        ],
    )?;
    if out.trim().is_empty() {
        run_os(
            repo,
            &[
                OsString::from("diff"),
                OsString::from("--"),
                OsStr::new(path).to_os_string(),
            ],
        )
    } else {
        Ok(out)
    }
}

/// Staged-only diff (used for AI commit message + review scope).
pub fn diff_cached(repo: &Path, limit_bytes: usize) -> Result<String> {
    let out = run(repo, &["diff", "--cached", "--stat", "--patch"])?;
    Ok(truncate_bytes(&out, limit_bytes))
}

/// Uncommitted diff (staged + unstaged) truncated for AI review.
pub fn diff_uncommitted(repo: &Path, limit_bytes: usize) -> Result<String> {
    let out = run(repo, &["diff", "HEAD", "--stat", "--patch"])?;
    Ok(truncate_bytes(&out, limit_bytes))
}

/// Diff of a range, e.g. `master...HEAD` for Agent Review.
pub fn diff_range(repo: &Path, range: &str, limit_bytes: usize) -> Result<String> {
    let out = run(repo, &["diff", "--stat", "--patch", range])?;
    Ok(truncate_bytes(&out, limit_bytes))
}

fn truncate_bytes(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        return s.to_string();
    }
    // Cut on a char boundary.
    let mut end = limit;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…\n…(truncated, {} bytes total)", &s[..end], s.len())
}

// ---------------------------------------------------------------------------
// Staging (P0)
// ---------------------------------------------------------------------------

pub fn stage_files(repo: &Path, paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Ok(String::new());
    }
    let mut args = vec![OsString::from("add"), OsString::from("--")];
    args.extend(paths_as_os(paths));
    run_os(repo, &args)
}

pub fn stage_all(repo: &Path) -> Result<String> {
    run(repo, &["add", "-A"])
}

pub fn unstage_files(repo: &Path, paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Ok(String::new());
    }
    let mut args = vec![
        OsString::from("restore"),
        OsString::from("--staged"),
        OsString::from("--"),
    ];
    args.extend(paths_as_os(paths));
    run_os(repo, &args)
}

pub fn unstage_all(repo: &Path) -> Result<String> {
    run(repo, &["restore", "--staged", "."])
}

/// Discard working-tree changes for tracked paths (`git restore -- <paths>`).
pub fn discard_files(repo: &Path, paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Ok(String::new());
    }
    let mut args = vec![OsString::from("restore"), OsString::from("--")];
    args.extend(paths_as_os(paths));
    run_os(repo, &args)
}

/// Delete untracked files (`git clean -f -- <paths>`). Caller confirms first.
pub fn clean_untracked(repo: &Path, paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Ok(String::new());
    }
    let mut args = vec![
        OsString::from("clean"),
        OsString::from("-f"),
        OsString::from("--"),
    ];
    args.extend(paths_as_os(paths));
    run_os(repo, &args)
}

/// Discard everything (tracked restore + untracked clean). Dangerous: UI gates
/// behind a confirm + policy prompt.
pub fn discard_all(repo: &Path) -> Result<String> {
    run(repo, &["restore", "."])?;
    // Best effort: untracked files need -f; surface the first error.
    let _ = run(repo, &["clean", "-fd"]);
    Ok(String::new())
}

/// Apply a hunk patch to the index (`git apply --cached`). `patch` must be a
/// valid unified diff with `a/`/`b/` paths.
pub fn stage_hunk(repo: &Path, patch: &str) -> Result<String> {
    let mut cmd = git_cmd(repo);
    cmd.args(["apply", "--cached", "--unidiff-zero", "-"]);
    use std::io::Write;
    let mut child = cmd
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| DeepSeekCodeError::Other(format!("git apply failed to spawn: {e}")))?;
    if let Some(stdin) = child.stdin.take() {
        let mut stdin = stdin;
        let _ = stdin.write_all(patch.as_bytes());
    }
    let output = child
        .wait_with_output()
        .map_err(|e| DeepSeekCodeError::Other(format!("git apply failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(DeepSeekCodeError::Other(if stderr.is_empty() {
            "git apply --cached failed".to_string()
        } else {
            stderr
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

// ---------------------------------------------------------------------------
// Commit (P0)
// ---------------------------------------------------------------------------

pub fn commit(repo: &Path, message: &str, no_verify: bool) -> Result<String> {
    let message = message.trim();
    if message.is_empty() {
        return Err(DeepSeekCodeError::Other("commit message is empty".into()));
    }
    let mut args: Vec<OsString> = vec![OsString::from("commit"), OsString::from("-m")];
    args.push(OsStr::new(message).to_os_string());
    if no_verify {
        args.push(OsString::from("--no-verify"));
    }
    run_os(repo, &args)
}

pub fn commit_amend(repo: &Path, message: Option<&str>, no_verify: bool) -> Result<String> {
    let mut args: Vec<OsString> = vec![OsString::from("commit"), OsString::from("--amend")];
    if let Some(m) = message {
        if !m.trim().is_empty() {
            args.push(OsString::from("-m"));
            args.push(OsStr::new(m).to_os_string());
        } else {
            args.push(OsString::from("--no-edit"));
        }
    } else {
        args.push(OsString::from("--no-edit"));
    }
    if no_verify {
        args.push(OsString::from("--no-verify"));
    }
    run_os(repo, &args)
}

/// `git reset --soft HEAD~1` — Cursor "Undo Last Commit".
pub fn undo_last_commit(repo: &Path) -> Result<String> {
    run(repo, &["reset", "--soft", "HEAD~1"])
}

// ---------------------------------------------------------------------------
// Branch (P2)
// ---------------------------------------------------------------------------

pub fn branch_list_verbose(repo: &Path) -> Result<String> {
    run(
        repo,
        &[
            "branch",
            "-vv",
            "--all",
            "--no-color",
            "--sort=-committerdate",
        ],
    )
}

pub fn current_branch(repo: &Path) -> Result<String> {
    run(repo, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|s| s.trim().to_string())
}

pub fn checkout_branch(repo: &Path, name: &str) -> Result<String> {
    run(repo, &["checkout", name])
}

pub fn checkout_new_branch(repo: &Path, name: &str, start_point: Option<&str>) -> Result<String> {
    if let Some(sp) = start_point {
        run(repo, &["checkout", "-b", name, sp])
    } else {
        run(repo, &["checkout", "-b", name])
    }
}

pub fn rename_branch(repo: &Path, old: &str, new: &str) -> Result<String> {
    run(repo, &["branch", "-m", old, new])
}

pub fn delete_branch(repo: &Path, name: &str, force: bool) -> Result<String> {
    if force {
        run(repo, &["branch", "-D", name])
    } else {
        run(repo, &["branch", "-d", name])
    }
}

pub fn set_upstream(repo: &Path, branch: &str, upstream: &str) -> Result<String> {
    run(repo, &["branch", "--set-upstream-to", upstream, branch])
}

pub fn cherry_pick(repo: &Path, sha: &str) -> Result<String> {
    run(repo, &["cherry-pick", sha])
}

pub fn cherry_pick_abort(repo: &Path) -> Result<String> {
    run(repo, &["cherry-pick", "--abort"])
}

pub fn revert_commit(repo: &Path, sha: &str, no_commit: bool) -> Result<String> {
    if no_commit {
        run(repo, &["revert", "--no-commit", sha])
    } else {
        run(repo, &["revert", sha])
    }
}

pub fn reset_soft(repo: &Path, target: &str) -> Result<String> {
    run(repo, &["reset", "--soft", target])
}

pub fn reset_hard(repo: &Path, target: &str) -> Result<String> {
    run(repo, &["reset", "--hard", target])
}

// ---------------------------------------------------------------------------
// Sync (P2)
// ---------------------------------------------------------------------------

pub fn fetch_all(repo: &Path) -> Result<String> {
    run(repo, &["fetch", "--all", "--prune"])
}

pub fn pull_rebase(repo: &Path) -> Result<String> {
    run(repo, &["pull", "--rebase", "--autostash"])
}

pub fn pull(repo: &Path) -> Result<String> {
    run(repo, &["pull"])
}

pub fn push(repo: &Path) -> Result<String> {
    run(repo, &["push"])
}

pub fn push_upstream(repo: &Path, remote: &str, branch: &str) -> Result<String> {
    run(repo, &["push", "-u", remote, branch])
}

pub fn push_force_with_lease(repo: &Path) -> Result<String> {
    run(repo, &["push", "--force-with-lease"])
}

/// `HEAD...@{u}` ahead/behind counts. Returns (ahead, behind). No upstream
/// yields (0, 0) instead of an error so the UI can show "no upstream".
pub fn ahead_behind(repo: &Path) -> (u32, u32) {
    let out = run(
        repo,
        &["rev-list", "--left-right", "--count", "HEAD...@{u}"],
    )
    .unwrap_or_default();
    let mut it = out.split_whitespace();
    let ahead = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let behind = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (ahead, behind)
}

// ---------------------------------------------------------------------------
// History / graph (P2)
// ---------------------------------------------------------------------------

/// Machine-readable log for graph rendering. Unit separator keeps subjects
/// with spaces/pipes intact. `%D` carries decorations (branches/tags/HEAD).
pub fn log_pretty(repo: &Path, limit: usize) -> Result<String> {
    run(
        repo,
        &[
            "log",
            &format!("-n{limit}"),
            "--all",
            "--date-order",
            "--pretty=format:%H%x1f%P%x1f%an%x1f%ct%x1f%s%x1f%D",
            "--no-color",
        ],
    )
}

pub fn show_commit_stat(repo: &Path, sha: &str) -> Result<String> {
    run(repo, &["show", "--stat", "--oneline", "--no-color", sha])
}

pub fn commit_files(repo: &Path, sha: &str) -> Result<String> {
    run(
        repo,
        &["diff-tree", "--no-commit-id", "--name-status", "-r", sha],
    )
}

// ---------------------------------------------------------------------------
// Stash (P3)
// ---------------------------------------------------------------------------

pub fn stash_list(repo: &Path) -> Result<String> {
    run(
        repo,
        &["stash", "list", "--pretty=format:%gd%x1f%H%x1f%s%x1f%ct"],
    )
}

pub fn stash_push(repo: &Path, message: Option<&str>, include_untracked: bool) -> Result<String> {
    let mut cmd = git_cmd(repo);
    cmd.arg("stash").arg("push");
    if include_untracked {
        cmd.arg("-u");
    }
    if let Some(m) = message {
        if !m.trim().is_empty() {
            cmd.arg("-m").arg(m);
        }
    }
    let output = cmd
        .output()
        .map_err(|e| DeepSeekCodeError::Other(format!("git stash push failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(DeepSeekCodeError::Other(if stderr.is_empty() {
            "git stash push failed".into()
        } else {
            stderr
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn stash_apply(repo: &Path, ref_: &str) -> Result<String> {
    run(repo, &["stash", "apply", ref_])
}

pub fn stash_pop(repo: &Path, ref_: &str) -> Result<String> {
    run(repo, &["stash", "pop", ref_])
}

pub fn stash_drop(repo: &Path, ref_: &str) -> Result<String> {
    run(repo, &["stash", "drop", ref_])
}

// ---------------------------------------------------------------------------
// Remotes / tags (P3)
// ---------------------------------------------------------------------------

pub fn remote_list(repo: &Path) -> Result<String> {
    run(repo, &["remote", "-v"])
}

pub fn tag_list(repo: &Path, limit: usize) -> Result<String> {
    run(repo, &["tag", "--list", "--sort=-creatordate"])
        .map(|s| s.lines().take(limit).collect::<Vec<_>>().join("\n"))
}

// ---------------------------------------------------------------------------
// Conflict helpers (P1)
// ---------------------------------------------------------------------------

/// Paths with unmerged entries (`U` in either index/worktree slot).
pub fn conflicted_paths(repo: &Path) -> Result<Vec<String>> {
    let out = run(repo, &["diff", "--name-only", "--diff-filter=U"])?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect())
}

pub fn resolve_with_ours(repo: &Path, paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Ok(String::new());
    }
    let mut args = vec![
        OsString::from("checkout"),
        OsString::from("--ours"),
        OsString::from("--"),
    ];
    args.extend(paths_as_os(paths));
    run_os(repo, &args)
}

pub fn resolve_with_theirs(repo: &Path, paths: &[String]) -> Result<String> {
    if paths.is_empty() {
        return Ok(String::new());
    }
    let mut args = vec![
        OsString::from("checkout"),
        OsString::from("--theirs"),
        OsString::from("--"),
    ];
    args.extend(paths_as_os(paths));
    run_os(repo, &args)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_keeps_char_boundary() {
        let s = "abc中文def";
        let out = truncate_bytes(s, 5);
        assert!(out.contains("…"));
    }

    /// End-to-end through a real temp repo (skipped when git is missing).
    #[test]
    fn stage_commit_undo_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        if run(repo, &["init"]).is_err() {
            return;
        }
        let _ = run(repo, &["config", "user.email", "test@example.com"]);
        let _ = run(repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
        stage_files(repo, &["a.txt".to_string()]).unwrap();
        commit(repo, "add a", false).unwrap();
        std::fs::write(repo.join("b.txt"), "world\n").unwrap();
        stage_files(repo, &["b.txt".to_string()]).unwrap();
        commit(repo, "add b", false).unwrap();
        let log = log_pretty(repo, 5).unwrap();
        assert!(log.contains("add b"));
        undo_last_commit(repo).unwrap();
        // After undo, b.txt is staged again (soft reset).
        let status = status_porcelain_v2(repo).unwrap();
        assert!(status.contains("b.txt"));
    }

    #[test]
    fn discard_all_restores_tracked_and_deletes_untracked() {
        // The exact code path behind the "Discard all → OK" confirm card.
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        if run(repo, &["init"]).is_err() {
            return;
        }
        let _ = run(repo, &["config", "user.email", "test@example.com"]);
        let _ = run(repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("a.txt"), "v1\n").unwrap();
        stage_files(repo, &["a.txt".to_string()]).unwrap();
        commit(repo, "base", false).unwrap();
        std::fs::write(repo.join("a.txt"), "dirty\n").unwrap();
        std::fs::write(repo.join("junk.txt"), "junk\n").unwrap();
        assert!(has_uncommitted_changes(repo).unwrap());
        discard_all(repo).unwrap();
        assert!(!has_uncommitted_changes(repo).unwrap());
        // NOTE: compare trimmed — Windows autocrlf may restore CRLF bytes.
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).unwrap().trim(),
            "v1"
        );
        assert!(!repo.join("junk.txt").exists());
    }

    #[test]
    fn staged_names_reflects_real_index() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        if run(repo, &["init"]).is_err() {
            return;
        }
        let _ = run(repo, &["config", "user.email", "test@example.com"]);
        let _ = run(repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("a.txt"), "v1\n").unwrap();
        // Unstaged change: index empty, tree dirty.
        assert!(staged_names(repo).unwrap().is_empty());
        assert!(has_uncommitted_changes(repo).unwrap());
        stage_files(repo, &["a.txt".to_string()]).unwrap();
        assert_eq!(staged_names(repo).unwrap(), vec!["a.txt".to_string()]);
        commit(repo, "base", false).unwrap();
        // Clean tree after commit.
        assert!(staged_names(repo).unwrap().is_empty());
        assert!(!has_uncommitted_changes(repo).unwrap());
    }

    #[test]
    fn stash_push_and_drop_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        if run(repo, &["init"]).is_err() {
            return;
        }
        let _ = run(repo, &["config", "user.email", "test@example.com"]);
        let _ = run(repo, &["config", "user.name", "Test"]);
        std::fs::write(repo.join("a.txt"), "v1\n").unwrap();
        stage_files(repo, &["a.txt".to_string()]).unwrap();
        commit(repo, "base", false).unwrap();
        std::fs::write(repo.join("a.txt"), "v2\n").unwrap();
        stash_push(repo, Some("wip"), false).unwrap();
        let list = stash_list(repo).unwrap();
        assert!(list.contains("wip"));
        stash_drop(repo, "stash@{0}").unwrap();
        assert!(stash_list(repo).unwrap().trim().is_empty());
    }
}
