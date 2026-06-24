//! A rust library that uses git to maintain a set of patch files against a repo.
//!
//! # See also
//! - [Arch Build System Patching](https://wiki.archlinux.org/index.php/Patching_packages)
//! - The [Paper](https://github.com/PaperMC/Paper) patching system
//!   - See [paperweight](https://github.com/PaperMC/paperweight) gradle plugin
//!   - [old `rebuildPatches.sh`](https://github.com/PaperMC/Paper-archive/blob/96f8b1a/scripts/rebuildPatches.sh)
//!   - [old `applyPatches.sh`](https://github.com/PaperMC/Paper-archive/blob/668ad2c/scripts/applyPatches.sh)
#![cfg_attr(use_error_backtrace, feature(error_generic_member_access))]

pub mod config;
pub mod engine;
pub(crate) mod utils;
pub mod vcs;
