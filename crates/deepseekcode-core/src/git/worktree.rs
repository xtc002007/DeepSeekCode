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
    let root = discover_git_root(path)?;
    let mut cmd = Command::new("git");
    crate::process::hide_console(&mut cmd);
    let branch = cmd
        .args([
            "-C",
            &root.to_string_lossy(),
            "rev-parse",
            "--abbrev-ref",
            "HEAD",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
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

    pub fn list(&self) -> Result<Vec<WorktreeEntry>, deepseekcode_types::DeepSeekCodeError> {
        let output = self.git(&["worktree", "list", "--porcelain"])?;
        Ok(parse_worktree_porcelain(&output))
    }

    pub fn create(
        &self,
        branch: Option<&str>,
    ) -> Result<WorktreeEntry, deepseekcode_types::DeepSeekCodeError> {
        if !self.is_git_repo() {
            return Err(deepseekcode_types::DeepSeekCodeError::Other(
                "not a git repository".into(),
            ));
        }

        let name = ulid::Ulid::new().to_string();
        let worktree_dir = self
            .repo_root
            .join(".deepseekcode")
            .join("worktrees")
            .join(&name);
        std::fs::create_dir_all(&worktree_dir)
            .map_err(deepseekcode_types::DeepSeekCodeError::Io)?;

        let mut args: Vec<&str> = vec!["worktree", "add"];
        if let Some(branch_name) = branch {
            args.extend(["-b", branch_name]);
            args.push(worktree_dir.to_str().unwrap_or_default());
        } else {
            args.extend([
                "--detach",
                worktree_dir.to_str().unwrap_or_default(),
                "HEAD",
            ]);
        }

        self.git(&args)?;

        Ok(WorktreeEntry {
            path: worktree_dir.display().to_string(),
            head: None,
            branch: branch.map(String::from),
            bare: false,
        })
    }

    pub fn remove(&self, worktree_path: &str) -> Result<(), deepseekcode_types::DeepSeekCodeError> {
        self.git(&["worktree", "remove", "--force", worktree_path])?;
        Ok(())
    }

    pub fn prune(&self) -> Result<(), deepseekcode_types::DeepSeekCodeError> {
        self.git(&["worktree", "prune"])?;
        Ok(())
    }

    fn git(&self, args: &[&str]) -> Result<String, deepseekcode_types::DeepSeekCodeError> {
        let mut cmd = Command::new("git");
        cmd.args(args).current_dir(&self.repo_root);
        crate::process::hide_console(&mut cmd);
        let output = cmd.output().map_err(|e| {
            deepseekcode_types::DeepSeekCodeError::Other(format!("git command failed: {e}"))
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(deepseekcode_types::DeepSeekCodeError::Other(format!(
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
