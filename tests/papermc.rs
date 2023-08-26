//! Tests gitpatcher by applying all the patches in
//! the [PaperMC repo](https://github.com/PaperMC/Paper).
//!
//! The project uses a custom [Gradle](https://gradle.org) plugin
//! called [paperweight](https://github.com/PaperMC/paperweight)
//! to apply their patches.
//! As far as I can tell, it is based on the `git` binary.
//!
//! This test compares the execution of `gitpatcher` against
//! the PaperMC implementation.
//! This is potentially a very expensive test because in addition
//! to running an entire external build process in Java,
//! it may require downlading several hundered MBs of jarfiles and git repositories.
//!
//! For this reason, the test is marked `#[ignore]` by default
//! and must be explicitly requested.
use anyhow::Context;

mod paper;

#[test]
#[ignore]
pub fn compare_paperweight_apply() -> anyhow::Result<()> {
    let root_work_dir = testdir::testdir!();
    let paper_repo_dir = root_work_dir.join("Paper");
    let paper_repo =
        paper::repo::setup_repo(&paper_repo_dir).context("Failed to setup paper repository")?;
    assert_eq!(paper_repo.workdir(), Some(&*paper_repo_dir));
    let paper_work = paper_repo.workdir();
    Ok(())
}
