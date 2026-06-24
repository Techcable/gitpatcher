use std::path::PathBuf;

/// Get the application config dir, typically `~/.config/$APP_NAME`.
pub fn config_dir() -> std::io::Result<PathBuf> {
    Ok(home_dir()?.join(".config/gitpatcher"))
}

/// Get the user's home directory, returning an error if unable to detect it.
pub fn home_dir() -> std::io::Result<PathBuf> {
    std::env::home_dir().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "Could not resolve home dir"))
}
