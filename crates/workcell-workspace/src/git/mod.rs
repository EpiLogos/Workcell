mod command;
mod provider;
mod state;

pub use command::GitBranchFacts;
pub use state::{GitWorktreeWorkspaceProvider, GIT_WORKTREES_JOURNAL_FILE, GIT_WORKTREES_SCHEMA};

/// Deliverable branch facts for a worktree path (see [`command::branch_facts`]).
pub fn git_branch_facts(worktree: &std::path::Path) -> epilogos_workcell_core::Result<GitBranchFacts> {
    command::branch_facts(worktree)
}
