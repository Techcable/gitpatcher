use git2::Commit;
use std::ops::Range;

#[derive(Debug)]
pub struct CommitMessage<'a> {
    full: &'a str,
    summary_range: Range<usize>,
    tail_range: Range<usize>,
}
impl<'a> CommitMessage<'a> {
    #[inline]
    #[allow(dead_code)]
    pub fn full(&self) -> &'a str {
        self.full
    }
    #[inline]
    pub fn summary(&self) -> &'a str {
        &self.full[self.summary_range.clone()]
    }
    #[inline]
    #[allow(dead_code)]
    pub fn body(&self) -> &'a str {
        &self.full[self.tail_range.clone()]
    }
    pub fn parse(full: &'a str) -> Result<Self, InvalidCommitMessage> {
        if full.is_empty() {
            return Err(InvalidCommitMessage::EmptyMessage);
        }
        let summary_start = full
            .char_indices()
            .find(|&(_, c)| !c.is_whitespace())
            .ok_or(InvalidCommitMessage::BlankMessage)?
            .0;
        let summary_end = full.find('\n').unwrap_or(full.len());
        let potential_body = &full[summary_end..];
        // Body starts at the first non-whitespace char past summary
        let body_start = potential_body
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(0)
            + summary_end;
        // Strip trailing whitespace
        let body_end = match potential_body.rfind(|c: char| !c.is_whitespace()) {
            Some(non_ws_idx) => non_ws_idx + 1, // becasue this is exclusive, we need to offset by 1
            None => potential_body.len(),
        } + summary_end;
        Ok(CommitMessage {
            full,
            summary_range: summary_start..summary_end,
            tail_range: body_start..body_end,
        })
    }
    pub fn from_commit(commit: &'a Commit) -> Result<Self, InvalidCommitMessage> {
        Self::parse(commit.message().ok_or(InvalidCommitMessage::InvalidUtf8)?)
    }

    pub fn patch_file_name(&self, patch_no: u32) -> String {
        assert!(patch_no >= 1);
        const MAX_LENGTH: usize = 52;
        let mut sanitized_name = String::new();
        let mut chars = self.summary().chars().peekable();
        while let Some(c) = chars.next() {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
                sanitized_name.push(c);
            } else if c == '(' && chars.peek() == Some(&')') {
                assert_eq!(chars.next(), Some(')'))
                // Ignore paired parens ()
            } else if !sanitized_name.ends_with('-') {
                sanitized_name.push('-');
            }
        }
        // Strip trailing '.' && '-'
        sanitized_name.truncate(sanitized_name.rfind(|c| c != '.' && c != '-').unwrap_or(0) + 1);
        // Strip leading '-'
        let first_valid = sanitized_name
            .find(|c| c != '-')
            .unwrap_or(sanitized_name.len());
        sanitized_name.drain(0..first_valid);
        sanitized_name.truncate(MAX_LENGTH);
        format!("{:04}-{}.patch", patch_no, sanitized_name)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidCommitMessage {
    #[error("Invalid UTF8 in commit message")]
    InvalidUtf8,
    /// Indicates that a message was completely empty (zero-length)
    #[error("Empty commit message")]
    EmptyMessage,
    /// Indicates that a message only contained whitespace
    #[error("Blank commit message (only whitespace)")]
    BlankMessage,
}

#[cfg(test)]
mod test {
    use crate::format_patches::format::CommitMessage;

    #[test]
    fn test_patch_file_name() {
        fn patch_file_name(t: &str, index: u32) -> String {
            CommitMessage::parse(t).unwrap().patch_file_name(index)
        }
        // Testing against PaperMC patch names
        assert_eq!(patch_file_name("POM Changes", 1), "0001-POM-Changes.patch");
        assert_eq!(
            patch_file_name("Version Command 2.0", 8),
            "0008-Version-Command-2.0.patch"
        );
        assert_eq!(
            patch_file_name(
                "Add methods for working with arrows stuck in living entities",
                20
            ),
            "0020-Add-methods-for-working-with-arrows-stuck-in-living-.patch"
        );
        assert_eq!(
            patch_file_name("Use ASM for event executors.", 22),
            "0022-Use-ASM-for-event-executors.patch"
        );
        assert_eq!(
            patch_file_name("Entity AddTo/RemoveFrom World Events", 28),
            "0028-Entity-AddTo-RemoveFrom-World-Events.patch"
        );
        assert_eq!(
            patch_file_name("Add MetadataStoreBase.removeAll(Plugin)", 31),
            "0031-Add-MetadataStoreBase.removeAll-Plugin.patch"
        );
        assert_eq!(
            patch_file_name("Make /plugins list alphabetical", 64),
            "0064-Make-plugins-list-alphabetical.patch"
        );
        assert_eq!(
            patch_file_name("Enderman.teleportRandomly()", 94),
            "0094-Enderman.teleportRandomly.patch"
        );
        assert_eq!(
            patch_file_name("Location.isChunkLoaded() API", 96),
            "0096-Location.isChunkLoaded-API.patch",
        );
        assert_eq!(
            patch_file_name("Add World.getEntity(UUID) API", 116),
            "0116-Add-World.getEntity-UUID-API.patch"
        );
        assert_eq!(
            patch_file_name("Performance & Concurrency Improvements to Permissions", 149),
            "0149-Performance-Concurrency-Improvements-to-Permissions.patch"
        );
        assert_eq!(
            patch_file_name("Here's Johnny!", 159),
            "0159-Here-s-Johnny.patch"
        );
        // Server patches
        assert_eq!(
            patch_file_name("Add ability to configure frosted_ice properties", 95),
            "0095-Add-ability-to-configure-frosted_ice-properties.patch"
        );
        // DuckLogic patches
        assert_eq!(
            patch_file_name("[sys] Declare DuckLogic-internal methods for list", 1),
            "0001-sys-Declare-DuckLogic-internal-methods-for-list.patch"
        )
    }
}
