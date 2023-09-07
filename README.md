gitpatcher
==========
A rust library that uses git to maintain a set
of patch files against a submodule.

## Features
- Uses [libgit2](https://libgit2.org/) internally
- The patcher creates a single patch file per commit
- It automatically adds patch files to the parent repository
  - Internally filters out redundant changes in patches,
    to avoid committing unnecessary changes

## See also
- [Arch Build System Patching](https://wiki.archlinux.org/index.php/Patching_packages) 
- [Paper](https://github.com/PaperMC/Paper) patching system
  - [rebuildPatches.sh](https://github.com/PaperMC/Paper/blob/96f8b1a/scripts/rebuildPatches.sh)
  - [applyPatches.sh](https://github.com/PaperMC/Paper/blob/668ad2c/scripts/applyPatches.sh)
