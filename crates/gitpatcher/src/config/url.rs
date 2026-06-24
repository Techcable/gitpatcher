//! Defines the [`VcsUrl`] type.
use std::fmt::{Debug, Display, Formatter, Write};
use std::str::FromStr;
use std::sync::Arc;

use serde::Deserializer;

/// A URL for a git repository.
#[derive(Clone, Eq, PartialEq, Hash)]
pub struct VcsUrl(Arc<gix_url::Url>);
impl VcsUrl {
    pub(crate) fn from_gix(gix_url: gix_url::Url) -> Self {
        VcsUrl(Arc::new(gix_url))
    }
    pub(crate) fn as_gix(&self) -> &gix_url::Url {
        &self.0
    }
    /// Convert this url into a [`gix_url::Url`] instance.
    pub fn to_gix(&self) -> gix_url::Url {
        self.as_gix().clone()
    }
}
impl<'de> serde::Deserialize<'de> for VcsUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Avoid <gix_url::Url as Deserialize> as that parses from a struct, not a string
        let s = <String as serde::Deserialize>::deserialize(deserializer)?;
        let gix = gix_url::Url::from_bytes((&*s).into()).map_err(serde::de::Error::custom)?;
        // the Arc is purely an optimization to save a couple malloc() calls each clone
        // it doesn't affect the semantics of the program
        Ok(VcsUrl(Arc::new(gix)))
    }
}
/// Prints the URL in a quoted form.
impl Debug for VcsUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_char('"')?;
        struct EscapingWriter<'a, 'b> {
            out: &'a mut Formatter<'b>,
        }
        impl Write for EscapingWriter<'_, '_> {
            fn write_str(&mut self, s: &str) -> std::fmt::Result {
                s.escape_debug().try_for_each(|c| self.out.write_char(c))
            }
        }
        write!(EscapingWriter { out: f }, "{}", self.0)?;
        f.write_char('"')
    }
}
impl Display for VcsUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self.as_gix(), f)
    }
}
impl FromStr for VcsUrl {
    type Err = RepoUrlParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        gix_url::Url::from_bytes(s.as_ref())
            .map(Self::from_gix)
            .map_err(|cause| RepoUrlParseError { cause })
    }
}
#[derive(thiserror::Error, Debug)]
#[error(transparent)]
pub struct RepoUrlParseError {
    cause: gix_url::parse::Error,
}

#[cfg(test)]
mod tests {
    use super::VcsUrl;

    #[test]
    fn repo_url_debug() {
        assert_eq!(
            format!("{:?}", "https://github.com/".parse::<VcsUrl>().unwrap()),
            r#""https://github.com/""#,
        );
        assert_eq!(
            format!("{:?}", "https://github.com/\\a".parse::<VcsUrl>().unwrap()),
            r#""https://github.com/\\a""#,
        );
    }
}
