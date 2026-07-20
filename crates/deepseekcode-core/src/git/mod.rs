pub mod project;
pub mod worktree;

pub use project::{ProjectDirectories, ProjectDirectoryEntry, ProjectDirectoryStore};
pub use worktree::{
    discover_git_root, is_git_repository, GitWorktreeManager, WorktreeEntry, WorktreeInfo,
};
