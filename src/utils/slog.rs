use git2::Oid;
use slog::{Key, Record, Serializer};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct GitOidValue(pub Oid);

impl slog::Value for GitOidValue {
    fn serialize(
        &self,
        _record: &Record,
        key: Key,
        serializer: &mut dyn slog::Serializer,
    ) -> slog::Result {
        serializer.emit_arguments(key, &format_args!("{}", self.0))
    }
}

pub struct PathValue(std::path::PathBuf);
impl slog::Value for PathValue {
    fn serialize(
        &self,
        _record: &Record,
        key: Key,
        serializer: &mut dyn Serializer,
    ) -> slog::Result {
        serializer.emit_arguments(key, &format_args!("{}", self.0.display()))
    }
}

pub trait SlogValueAdapter {
    type Adapter: slog::Value;
    fn to_slog(&self) -> Self::Adapter;
}
macro_rules! slog_value_adapter {
    ({ $($target:ty);* } as $adapter:ty; |$this:ident| $convert:expr) => {
        $(impl SlogValueAdapter for $target {
            type Adapter = $adapter;
            #[inline]
            fn to_slog(&self) -> Self::Adapter {
                let $this : &Self = self;
                $convert
            }
        })*
    };
}
slog_value_adapter!({
    camino::Utf8Path;
    camino::Utf8PathBuf;
    std::path::Path;
    std::path::PathBuf
} as PathValue; |this| PathValue(this.into()));
slog_value_adapter!({ Oid } as GitOidValue; |this| GitOidValue(*this));
