use std::ffi::OsStr;
use std::process::Command;

use indexmap::IndexMap;
use slog::{Key, Logger, Record, Serializer};

pub trait DuctLoggingExt {
    fn log_on_spawn(&self, logger: &Logger) -> Self;
}
impl DuctLoggingExt for duct::Expression {
    fn log_on_spawn(&self, logger: &Logger) -> Self {
        let logger = logger.clone();
        self.before_spawn(move |command| {
            log_command(&logger, command);
            Ok(())
        })
    }
}

/// A [`slog::Value`] that logs information on a [`std::process::Command`].
#[non_exhaustive]
pub struct LoggedCommand<'a> {
    pub command: &'a Command,
    /// Include environment variables in the logged information.
    ///
    /// This is off by default as it bloats the logfiles.
    pub include_env: bool,
}
impl<'a> LoggedCommand<'a> {
    /// Log information on the specified command.
    pub fn new(cmd: &'a Command) -> Self {
        LoggedCommand {
            command: cmd,
            include_env: false,
        }
    }
}
impl<'a> From<&'a Command> for LoggedCommand<'a> {
    fn from(cmd: &'a Command) -> Self {
        Self::new(cmd)
    }
}
impl slog::Value for LoggedCommand<'_> {
    fn serialize(&self, record: &Record<'_>, key: Key, serializer: &mut dyn Serializer) -> slog::Result {
        fn into_string_lossy(s: &OsStr) -> String {
            s.to_string_lossy().into_owned()
        }
        #[derive(serde::Serialize, Clone)]
        struct SerializedInfo {
            program: String,
            args: Vec<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            cwd: Option<String>,
            #[serde(rename = "env", skip_serializing_if = "IndexMap::is_empty")]
            custom_envs: IndexMap<String, Option<String>>,
        }
        let value = slog::Serde(SerializedInfo {
            program: into_string_lossy(self.command.get_program()),
            args: self.command.get_args().map(into_string_lossy).collect(),
            cwd: self
                .command
                .get_current_dir()
                .map(std::path::Path::as_os_str)
                .map(into_string_lossy),
            custom_envs: if self.include_env {
                self.command
                    .get_envs()
                    .map(|(name, value)| (into_string_lossy(name), value.map(into_string_lossy)))
                    .collect::<IndexMap<_, _>>()
            } else {
                IndexMap::new()
            },
        });
        slog::Value::serialize(&value, record, key, serializer)
    }
}

/// Log a [`std::process::Command`] to a [`slog::Logger`] at the [`slog::Level::Debug`] level.
///
/// See [`LoggedCommand`] for more info and customization options.
pub fn log_command(logger: &Logger, command: &std::process::Command) {
    slog::debug!(
        logger,
        "Executing command";
        "cmd" => LoggedCommand::new(command),
    );
}
