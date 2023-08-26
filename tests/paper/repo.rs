use std::path::{Path, PathBuf};

use git2::Repository;

pub struct PaperRepo {
    paper_repo: Repository,
}
pub fn setup_repo(_target: &Path) -> anyhow::Result<Repository> {
    todo!()
}
