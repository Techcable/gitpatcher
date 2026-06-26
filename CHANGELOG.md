# Changelog

Notable changes to this project should be documented in this file.
Make sure it is up to date before performing a release.

New versions should follow the [Keep a Changelog](https://keepachangelog.com/en/2.0.0/),
although older versions descriptions were simply copied from github releases notes.

The "title" of each release should be its first line.
A title is required for publishing a github release, so all versions should have one.

## Unreleased

### Changes
- Commit lockfile to version control.
- Sync crate docs with README.md using `cargo-reedme`

## 0.2.5 - 2025-06-25
Avoid use of libgit2 for build-time version detection.

Improves the ability to cross-compile.

### Changes
- Avoid use of libgit2 at build time to detect CLI version.
  Instead call out to the git CLI. Only affects the CLI, not the library.

## 0.2.4 - 2026-06-24
Correctly resolve patchdir directory, relative to repo workdir.

A minor release with some bugfixes and lots of internal improvement.

### Fixes
- Correctly resolve patchdir directory, relative to repo workdir (1940a02)
- Better panic message for internal RememberLast.back out-of-bounds (38cf4ba)
- Fix outstanding clippy warnings (c201fda)

### Changes
- Upgrade dependencies (53acc1c)
  - Since this release spanned such a long development period, some updates commits were later superseded (ae8fc80, 6a7c163, 1ac6467, 64e7fd7)
- Dual-license as MIT/APACHE-2.0 instead of just MIT (427220f)
- Require a feature flag to be explciitly selected to enable error backtraces (removes build.rs auto-detection)

### Added (Internal)
- Include backtraces for "unexpected" `PatchError` (7ebb022)
- *Experimental*: Add test command to rebuild Paper patches (bf79706)
- Run TOML formatting with `taplo` (27ec318)
- check spelling with `typos` (03c96bd)
- Add `tasks.py` file for use with [pyinvoke](https://pyinvoke.org) (ff638c6)

## 0.2.3 - 2023-10-10
Move logic for `apply-all-patches` to the library.

Doesn't affect the command line interface very much, but changes the output by using slog instead of println!.

Update `vergen` to 8.2.5 to use `git2-0.18`.
This way we can avoid invoking the command line interface for version detection.

## 0.2.2 - 2023-08-24
This release fixes a minor issue with v0.2.0 compiling on stable.

For all other purposes, it should be equivalent to it.

## 0.2.1 - 2023-08-24
Fix version 0.2.0 compilation on stable

This release fixes a minor issue with v0.2.0 compiling on stable.

For all other purposes, it should be equivalent to it.

## 0.2.0 - 2023-08-24
Use diffy to apply patches instead of libgit2.

Using diffy allows for applying patches with incorrect line numbers.
This is what the standard `git-apply` command does and also GNU patch.
This feature was missing from recent versions of libgit2 (but appeared to be in old ones).
See PR #4 for more details.

This is by far the biggest change. Note that libgit2 is still a required dependency.
- Switch parsing from `regex` to `nom`
- Improve error messages (hopefully) by using `thiserror`
- By default, use static linking for dependencies
- Move parsing logic out of apply/regenerate module
   - Hopefully makes more usable as a library
- Use `vergen` to include `git-describe` info into clap CLI
   - Implement a `--version` flag for the CLI

**WARNING**: This release is effectively broken, because it does not support stable rust.
I foolishly published before running `./dist.sh`
For this reason, there are no published binaries and it has been yanked from <https://crates.io>.
Prefer v0.2.1, which differs only slightly and has included a fix.

## 0.1.2 - 2023-05-30
Update git2 crate from 0.15 -> 0.17.

**WARNING**: This is broken!

### Features
- Upgrades required libgit2 from 1.4 -> 1.6


### Internals
- Update to Rust 2021 edition
- Use byte strings for parsing email messages

## 0.1.1 - 2022-01-10
Work on updating dependencies

Update from chrono -> time and structopt -> clap3

Some work on reducing binary size. Still requires clap and regex :(

## 0.1.0 - 2026-06-05
Initial release

This is tested and confirmed to roundtrip a large set of patches (All of PaperMC).

I've been using this in DuckLogic for a while now, so it's reasonably stable.
