use serde::Deserialize;
use std::path::Path;

/// Configuration for Keeper of Keys.
///
/// This config is meant to be stored in ~/Library/Containers/<bundleid>/Data/config.toml.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct Config {
    /// List of keychain items that notifications should be
    /// suppressed for.
    ///
    /// Example: `handoff-own-encryption-key`
    pub(crate) ignored_items: Vec<String>,
}

impl Config {
    const FILE_NAME: &'static str = "config.toml";

    pub(crate) fn read_from_dir(data_dir: &Path) -> Self {
        let fallback = Config::default();
        match std::fs::read_to_string(data_dir.join(Self::FILE_NAME)) {
            Ok(val) => {
                if let Ok(config) = toml::from_str(&val) {
                    config
                } else {
                    log::warn!("incorrect config found, ignoring");
                    fallback
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log::debug!("config not found, ignoring");
                fallback
            }
            Err(e) => {
                log::warn!("failed to read config: {e}");
                fallback
            }
        }
    }

    pub(crate) fn setup_home_link(data_dir: &Path, home_dir: &Path) {
        let source = data_dir.join(Self::FILE_NAME);

        // No config anyway, nothing to do.
        if !source.exists() {
            return;
        }

        let mut dest = home_dir.to_path_buf();
        dest.push(".config");

        // We are not important enough to make this if it doesn't exist already.
        if !dest.exists() {
            return;
        }

        dest.push("keeper_of_keys");

        let _ = std::fs::create_dir(&dest);

        dest.push(Self::FILE_NAME);

        // Already done, nothing to do
        if dest.is_symlink() {
            return;
        }

        if let Err(e) = std::os::unix::fs::symlink(source, dest) {
            log::warn!("failed to make config convenience link: {e}");
        }
    }
}
