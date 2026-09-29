//! Git surface. Per-operation product logging lives in `project.rs`
//! (STORAGE target, project_path/directory) and `worktree.rs` (STORAGE
//! target, repo/branch, warn/error on failure); this module only re-exports
//! so it emits no separate `tracing!` sites.
pub mod project;
pub mod worktree;

pub use project::{ProjectDirectories, ProjectDirectoryEntry, ProjectDirectoryStore};
pub use worktree::{
    discover_git_root, is_git_repository, GitWorktreeManager, WorktreeEntry, WorktreeInfo,
};
