import os
import shlex
import sys

from invoke import Collection, task

HAS_COLORS: bool = (sys.stderr.isatty() or os.getenv("CLICOLOR_FORCE")) and not os.getenv("NO_COLOR")


def apply_colors(msg: object, /, *, code: str) -> str:
    if HAS_COLORS:
        return f"\x1b[{code}m{msg}\x1b[0m"
    else:
        return str(msg)


def log_info(msg: object):
    print(
        apply_colors("INFO:", code="1;32"),
        apply_colors(msg, code="1"),
    )


@task
def test(ctx):
    check(ctx, format=False)
    ctx.run("cargo nextest run --workspace", pty=True)
    # needed because nextest doesn't support doctests
    ctx.run("cargo test --doc --workspace", pty=True)
    run_format(ctx, check=True)


@task
def check(ctx, format=True):
    clippy(ctx)
    # need to exclude gitpatcher-bin due to "output filename collision"
    # The gitpatcher binary conflicts with the gitpatcher library.
    # See rust-lang/cargo#6313 for details
    ctx.run("cargo +nightly doc --document-private-items --no-deps --workspace --exclude gitpatcher-bin --all-features")
    # separately check gitpatcher-bin docs
    ctx.run("cargo +nightly doc --document-private-items --no-deps -p gitpatcher-bin --all-features")
    ctx.run("cargo shear")
    # by default, check formatting as well
    if format:
        run_format(ctx, check=True)


REEDME_SYNC_PKGS = ["gitpatcher", "test-paper-patch"]
"""
Packages to sync with cargo-reedme.

We only want to sync the readmes of a subset of packages.
Some crates don't have dedicated README and trying to sync them will affect the root readme.
"""


@task
def cargo_reedme(ctx, check=False):
    args = ["cargo", "reedme"]
    if check:
        args.append("--check")
    for pkg in REEDME_SYNC_PKGS:
        args.extend(("-p", pkg))
    ctx.run(shlex.join(args))


@task
def clippy(ctx):
    ctx.run("cargo clippy --workspace --all-targets", pty=True)


@task(name="format")
def run_format(ctx, check=False):
    verb = "Checking" if check else "Fixing"
    log_info(f"{verb} formatting")
    maybe_check = " --check" if check else ""
    maybe_fix = " --fix" if not check else ""
    ctx.run("cargo +nightly fmt --all" + maybe_check)
    ctx.run("tombi format" + maybe_check)
    # cargo-sort is currently disabled as it causes excessive rebase conflicts
    # ctx.run("cargo sort --grouped --no-format --workspace" + maybe_check)a

    # need python format for invoke.py
    ctx.run("ruff format" + maybe_check)
    ctx.run("ruff check --select=I" + maybe_fix)  # works like isort
    check_spelling(ctx, fix=False)


TYPOS_VER = "1.45"  # pinned to avoid update breakage


@task(name="typos")
def check_spelling(ctx, fix=False):
    maybe_write = " --write-changes" if fix else ""
    ctx.run(f"uvx typos@{TYPOS_VER}" + maybe_write)


ns = Collection(test, check, clippy, cargo_reedme, run_format, check_spelling)
ns.configure(
    {
        "run": {
            "echo": True,
            "env": {"CLICOLOR_FORCE": "1" if HAS_COLORS else "0"},
        }
    }
)
