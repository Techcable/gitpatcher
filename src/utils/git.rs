//! Utilities for working with [libgit2](https://libgit2.org)

use std::path::{Component, Path, PathBuf};

use git2::{FileMode, Object, ObjectType, Oid, Repository};

pub fn create_nested_tree<'repo>(
    repo: &'repo Repository,
    path: &Path,
    root_object: &Object<'repo>,
    mut current_mode: FileMode,
) -> Result<git2::Tree<'repo>, NestedTreeError> {
    if path.is_absolute() {
        return Err(NestedTreeError::AbsolutePathError { path: path.into() });
    }
    let expected_object_kind = match current_mode {
        // TODO: I think it's possible to support some of these
        FileMode::Unreadable | FileMode::Link | FileMode::Commit => {
            return Err(NestedTreeError::InvalidObjectMode {
                mode: current_mode,
                object_id: root_object.id(),
                actual_kind: root_object.kind(),
                cause: None,
            })
        }
        FileMode::Tree => ObjectType::Tree,
        FileMode::Blob | FileMode::BlobGroupWritable | FileMode::BlobExecutable => ObjectType::Blob,
    };
    let mut current_entry = root_object.clone();
    if current_entry.kind() != Some(expected_object_kind) {
        return Err(NestedTreeError::UnexpectedObjectKind {
            expected: expected_object_kind,
            role: "specified",
            actual_kind: root_object.kind(),
            object_id: root_object.id(),
        });
    }
    let mut components = path.components().rev().peekable();
    if components.peek().is_none() {
        return Err(NestedTreeError::BlankPath {
            path: path.to_path_buf(),
        });
    }
    for component in components {
        let Component::Normal(normal) = component else {
            return Err(NestedTreeError::BadPathComponent {
                component_debug: format!("{:?}", component),
                path: path.to_path_buf(),
            });
        };
        let mut builder = repo.treebuilder(None)?;
        builder.insert(normal, current_entry.id(), i32::from(current_mode))?;
        current_entry = repo.find_object(builder.write()?, Some(ObjectType::Tree))?;
        current_mode = FileMode::Tree;
    }
    current_entry
        .into_tree()
        .map_err(|actual_result| NestedTreeError::UnexpectedObjectKind {
            role: "result",
            expected: ObjectType::Tree,
            actual_kind: actual_result.kind(),
            object_id: actual_result.id(),
        })
}

#[derive(Debug, thiserror::Error)]
pub enum NestedTreeError {
    #[error("Cannot create tree with absolute path: {}", path.display())]
    AbsolutePathError { path: PathBuf },
    #[error("Unexpected path component `{component_debug}` in path {}", path.display())]
    BadPathComponent {
        component_debug: String,
        path: PathBuf,
    },
    #[error("Specified blank path: `{}`", path.display())]
    BlankPath { path: PathBuf },
    #[error(
        "Invalid file mode {mode:?} specified for object {object_id} (actually {actual:?})",
        actual = crate::utils::OptionDebugWithoutSome(actual_kind.as_ref())
    )]
    InvalidObjectMode {
        mode: FileMode,
        actual_kind: Option<ObjectType>,
        object_id: git2::Oid,
        #[source]
        cause: Option<git2::Error>,
    },
    #[error(
        "Expected an object of kind {expected:?} for {role} object {object_id}, but got {actual:?}",
        actual = crate::utils::OptionDebugWithoutSome(actual_kind.as_ref())
    )]
    UnexpectedObjectKind {
        expected: ObjectType,
        actual_kind: Option<ObjectType>,
        object_id: Oid,
        role: &'static str,
    },
    #[error(transparent)]
    GitError(#[from] git2::Error),
}
