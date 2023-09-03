from __future__ import annotations

import functools
import sys
from pathlib import Path

from subprocess import run, check_call

import click
try:
    import pygit2
except ImportError:
    click.echo(click.style('ERROR', fg='red', bold=True) + ": Failed to import required dependency `pygit2`", err=True)
    raise click.Abort

from pygit2 import Repository

def relativize(p: Path | str, *, resolve: bool = False) -> Path:
    resolved: Path
    if resolve:
        resolved = Path(p).absolute()
    else:
        resolved = Path(p)
    return resolved.relative_to(Path.cwd())

@functools.cache
def determine_this_repo() -> tuple[Repository, Path]:
    this_file_path = Path(__file__).absolute()
    gitpatcher_repo = pygit2.Repository(this_file_path)
    repo_workdir = Path(gitpatcher_repo.workdir).absolute()
    assert repo_workdir.is_dir(), repo_workdir
    joined_path = repo_workdir / f"scripts/{this_file_path.name}"
    assert joined_path == this_file_path, (this_file_path, joined_path)
    return gitpatcher_repo, repo_workdir


def clone_paper_repo(original_repo: Repository, result_dir: Path) -> Repository:
    print("Closing (local) copy of papermc repo into", result_dir)
    assert not result_dir.is_dir(), result_dir
    return pygit2.clone_repository(original_repo, result_dir)


@click.command()
@click.option(
        'original_papermc_repo_dir',
        '--papermc-repo',
        required=True,
        type=click.Path(type=Path, is_dir=True, exists=True),
        help="The original PaperMC repository to use",
)
def compare(original_papermc_repo_dir, shutil=None):
    """
    A script to compare the gitpatcher output against the 'paperweight' system used by PaperMC.

    See https://papermc.io/ for more info on the PaperMC system.
    """
    try:
        original_papermc_repo = Repository(original_papermc_repo_dir, flags=pygit2.GIT_REPOSITORY_OPEN_NO_SEARCH)
    except pygit2.GitError as e:
        raise click.ClickException(f'Invalid repo for `--papermc-repo`: {original_papermc_repo_dir}') from e
    gitpatcher_repo, repo_workdir = determine_this_repo()
    if Path.cwd() != repo_workdir:
        raise click.ClickException(f"Current working directory must be {repo_workdir}: {Path.cwd()}")

    papermc_work_dir = relativize(repo_workdir / "work/papermc-compare")
    cloned_repo_dir = papermc_work_dir / "PaperMC-local.git"
    cloned_repo: Repository
    if cloned_repo_dir.is_dir():
        print("Local PaperMC repo already exists:", cloned_repo_dir)
        match click.prompt(
            "Local PaperMC repo already exists. Do you want to wipe or reuse the directory?",
            type=click.Choice(('wipe', 'reuse')),
            confirmation_prompt=True
        ):
            case 'wipe':
                print("Wiping existing repo")
                shutil.rmtree(cloned_repo_dir)
                cloned_repo = clone_paper_repo(original_papermc_repo, cloned_repo_dir)
            case 'reuse':
                print("Reusing existing repo:", cloned_repo_dir)
                cloned_repo = Repository(cloned_repo_dir, flags=pygit2.GIT_REPOSITORY_OPEN_NO_SEARCH)
            case other:
                raise AssertionError(other)
    else:
        print(f"Missing (local) copy of PaperMC repo (should be in {cloned_repo_dir})")
        click.confirm('Do you want to clone the PaperMC repository?', abort=True)
        cloned_repo = clone_paper_repo(original_papermc_repo, cloned_repo_dir)
    assert isinstance(cloned_repo, pygit2.Repository), cloned_repo
    # TODO: Finish this
    print("TODO:", "Finish implementing this")
    raise NotImplementedError("TODO")


if __name__ == '__main__':
    compare()


