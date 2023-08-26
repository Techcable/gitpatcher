//! Tests for patches with bad line numbers.
//!
//! Tests that both applying patches can succeed anyways (based on the context)
//! and that regenerating the patches doesn't generate unecessary changes.
use std::env;
use std::path::{Path, PathBuf};

use git2::{Repository, ResetType};
use gitpatcher::apply_patches::EmailMessage;

#[test]
pub fn approx_pi_patch() -> anyhow::Result<()> {
    let test_data_tempdir: PathBuf = testdir::testdir!();
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let test_data_dir = manifest_dir.join("tests/data");
    let approx_pi_file = test_data_dir.join("approx_pi.rs");
    let approx_pi_patch_path = test_data_dir.join("approx_pi.rs.patch");
    let approx_pi_patch_contents = std::fs::read_to_string(&approx_pi_patch_path)?;
    let approx_pi_patch_email = EmailMessage::parse(&approx_pi_patch_contents)?;
    let repo_path = test_data_tempdir.join("approx_pi_repo");
    let repo = Repository::init_opts(
        &repo_path,
        git2::RepositoryInitOptions::new().no_reinit(true),
    )?;
    assert_eq!(repo.state(), git2::RepositoryState::Clean);
    assert!(repo.is_empty()?, "Repo should be empty");
    let repo_workdir = repo.workdir().unwrap();
    let approx_pi_repo_file = repo_workdir.join(approx_pi_file.file_name().unwrap());
    std::fs::copy(&approx_pi_file, &approx_pi_repo_file)?;
    repo.index()?
        .add_path(&approx_pi_repo_file.strip_prefix(repo_workdir)?)?;
    let tree_id = repo.index()?.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let sig = git2::Signature::now("dummy", "dummy@dumb.gov")?;
    let commit_id = repo.commit(Some("HEAD"), &sig, &sig, "Initial commit", &tree, &[])?;
    let commit = repo.find_commit(commit_id)?;
    // Hard reset
    repo.reset(commit.as_object(), ResetType::Hard, None)?;
    approx_pi_patch_email.apply_commit(&repo)?;
    Ok(())
}
