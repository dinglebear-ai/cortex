//! Git repository and worktree observation primitives.

pub mod discovery;
pub mod porcelain;
pub(crate) mod reconcile;

#[cfg(test)]
pub(crate) mod test_support;
