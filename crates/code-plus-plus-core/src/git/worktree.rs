use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Git worktree / repository metadata for a directory.
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub root: PathBuf,
    pub branch: Option<String>,
}

/// Walk parents until a `.git` directory or file is found.
pub fn discover_git_root(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        let git_marker = current.join(".git");
        if git_marker.exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

pub fn is_git_repository(path: &Path) -> bool {
    discover_git_root(path).is_some()
}

pub fn worktree_info(path: &Path) -> Option<WorktreeInfo> {
    tracing::debug!(
        target: code_plus_plus_telemetry::STORAGE,
        run_id = %code_plus_plus_telemetry::current_run_id(),
        path = %path.display(),
        "worktree_info queried"
    );
    let root = discover_git_root(path)?;
    let mut cmd = Command::new("git");
    crate::process::hide_console(&mut cmd);
    // Pass the repo path as OsStr (no UTF-8 loss) and decode branch as
    // lossy UTF-8 instead of silently dropping non-UTF-8 output.
    let branch = cmd
        .arg("-C")
        .arg(root.as_os_str())
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    Some(WorktreeInfo { root, branch })
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorktreeEntry {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub bare: bool,
}

pub struct GitWorktreeManager {
    repo_root: PathBuf,
}

impl GitWorktreeManager {
    pub fn new(repo_root: impl AsRef<Path>) -> Self {
        Self {
            repo_root: repo_root.as_ref().to_path_buf(),
        }
    }

    pub fn is_git_repo(&self) -> bool {
        is_git_repository(&self.repo_root)
    }

    pub fn list(&self) -> Result<Vec<WorktreeEntry>, code_plus_plus_types::CodePlusPlusError> {
        tracing::debug!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            repo = %self.repo_root.display(),
            "git worktree list started"
        );
        let output = self
            .git(&["worktree", "list", "--porcelain"])
            .map_err(|e| {
                tracing::warn!(
                    target: code_plus_plus_telemetry::STORAGE,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    repo = %self.repo_root.display(),
                    error = %e,
                    "git worktree list failed"
                );
                e
            })?;
        Ok(parse_worktree_porcelain(&output))
    }

    pub fn create(
        &self,
        branch: Option<&str>,
    ) -> Result<WorktreeEntry, code_plus_plus_types::CodePlusPlusError> {
        tracing::info!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            repo = %self.repo_root.display(),
            branch = ?branch,
            "git worktree create started"
        );
        if !self.is_git_repo() {
            tracing::warn!(
                target: code_plus_plus_telemetry::STORAGE,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                repo = %self.repo_root.display(),
                "git worktree create failed: not a git repository"
            );
            return Err(code_plus_plus_types::CodePlusPlusError::Other(
                "not a git repository".into(),
            ));
        }

        let name = ulid::Ulid::new().to_string();
        let worktree_dir = self
            .repo_root
            .join(".code-plus-plus")
            .join("worktrees")
            .join(&name);
        // Do NOT pre-create the directory: `git worktree add` creates it.
        // Pre-creating makes git fail ("already exists") and leaves an empty
        // dir behind. On failure below we remove any partial dir git left.
        // Use OsStr args throughout so non-UTF-8 paths never silently become
        // "" via `to_str().unwrap_or_default()`.
        let result = self.git_add_worktree(branch, &worktree_dir);
        if let Err(e) = result {
            // Best-effort cleanup of a half-created worktree dir.
            let _ = std::fs::remove_dir_all(&worktree_dir);
            return Err(e);
        }

        Ok(WorktreeEntry {
            path: worktree_dir.display().to_string(),
            head: None,
            branch: branch.map(String::from),
            bare: false,
        })
    }

    pub fn remove(&self, worktree_path: &str) -> Result<(), code_plus_plus_types::CodePlusPlusError> {
        tracing::info!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            repo = %self.repo_root.display(),
            path = %worktree_path,
            "git worktree remove started"
        );
        self.git(&["worktree", "remove", "--force", worktree_path])
            .map_err(|e| {
                tracing::warn!(
                    target: code_plus_plus_telemetry::STORAGE,
                    run_id = %code_plus_plus_telemetry::current_run_id(),
                    repo = %self.repo_root.display(),
                    path = %worktree_path,
                    error = %e,
                    "git worktree remove failed"
                );
                e
            })?;
        Ok(())
    }

    pub fn prune(&self) -> Result<(), code_plus_plus_types::CodePlusPlusError> {
        tracing::info!(
            target: code_plus_plus_telemetry::STORAGE,
            run_id = %code_plus_plus_telemetry::current_run_id(),
            repo = %self.repo_root.display(),
            "git worktree prune started"
        );
        self.git(&["worktree", "prune"]).map_err(|e| {
            tracing::warn!(
                target: code_plus_plus_telemetry::STORAGE,
                run_id = %code_plus_plus_telemetry::current_run_id(),
                repo = %self.repo_root.display(),
                error = %e,
                "git worktree prune failed"
            );
            e
        })?;
        Ok(())
    }

    /// `git worktree add` with OsStr path handling (no UTF-8 loss).
    /// Separated so `create()` can clean up the dir on failure.
    fn git_add_worktree(
        &self,
        branch: Option<&str>,
        worktree_dir: &Path,
    ) -> Result<String, code_plus_plus_types::CodePlusPlusError> {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.repo_root);
        crate::process::hide_console(&mut cmd);
        // Build args as OsString so non-UTF-8 paths survive verbatim.
        let mut args: Vec<OsString> = vec!["worktree".into(), "add".into()];
        if let Some(branch_name) = branch {
            args.push("-b".into());
            args.push(OsStr::new(branch_name).to_os_string());
        } else {
            args.push("--detach".into());
        }
        args.push(worktree_dir.as_os_str().to_os_string());
        if branch.is_none() {
            args.push("HEAD".into());
        }
        // Log lossy only for diagnostics; the spawn uses OsStr verbatim.
        let log_args = args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ");
        let output = cmd.args(&args).output().map_err(|e| {
            code_plus_plus_types::CodePlusPlusError::Other(format!("git command failed: {e}"))
        })?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(code_plus_plus_types::CodePlusPlusError::Other(format!(
                "git {log_args} failed: {stderr}{stdout}"
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn git(&self, args: &[&str]) -> Result<String, code_plus_plus_types::CodePlusPlusError> {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(&self.repo_root);
        crate::process::hide_console(&mut cmd);
        let output = cmd.output().map_err(|e| {
            code_plus_plus_types::CodePlusPlusError::Other(format!("git command failed: {e}"))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(code_plus_plus_types::CodePlusPlusError::Other(format!(
                "git {} failed: {}{}",
                args.join(" "),
                stderr,
                stdout
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

fn parse_worktree_porcelain(output: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;

    for line in output.lines() {
        if line.starts_with("worktree ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            current = Some(WorktreeEntry {
                path: line.trim_start_matches("worktree ").to_string(),
                head: None,
                branch: None,
                bare: false,
            });
        } else if let Some(ref mut entry) = current {
            if line.starts_with("HEAD ") {
                entry.head = Some(line.trim_start_matches("HEAD ").to_string());
            } else if line.starts_with("branch ") {
                entry.branch = Some(
                    line.trim_start_matches("branch ")
                        .trim_start_matches("refs/heads/")
                        .to_string(),
                );
            } else if line == "bare" {
                entry.bare = true;
            }
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn discovers_nested_git_root() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        let root = discover_git_root(&dir.path().join("sub")).unwrap();
        assert_eq!(root, dir.path());
    }
}
