//! Clones, worktrees and the merge-base diff (design section 8.2).

pub mod diff;
pub mod worktree;

pub use diff::DiffIndex;
pub use worktree::{Checkout, Repos};
