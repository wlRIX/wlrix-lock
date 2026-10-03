// SPDX-License-Identifier: GPL-3.0-or-later
//! Where the config file is.
//!
//! The convention is the whole stack's and this module only follows it: a hand-edited file under
//! `$XDG_CONFIG_HOME/wlrix/`, with `/etc/wlrix/` consulted when the user has none of their own,
//! and **the first file found wins outright** -- nothing is merged. Copied from
//! `wlrix-bg/src/xdg.rs` with the file name changed; the lookup is duplicated in every component
//! because the repos build standalone, and this is one more copy rather than a new rule.
//!
//! The stem is `lock`, the binary's own name less the prefix, as `idle.toml` is `wlrix-idle`'s.

use std::path::{Path, PathBuf};

/// The hand-edited settings file, relative to a config directory.
pub const CONFIG_NAME: &str = "wlrix/lock.toml";
/// Consulted when the user has no config of their own.
const SYSTEM_CONFIG_DIR: &str = "/etc";

/// `$HOME`, or `None` when even that is unset.
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// `$XDG_CONFIG_HOME`, or `~/.config` as the spec says to assume.
pub fn user_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir));
    }
    home().map(|home| home.join(".config"))
}

/// Where to look for the settings file, most specific first.
pub fn config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = user_config_dir() {
        paths.push(dir.join(CONFIG_NAME));
    }
    paths.push(Path::new(SYSTEM_CONFIG_DIR).join(CONFIG_NAME));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_path_is_the_one_install_writes() {
        // `just install` puts the default at /etc/wlrix/lock.toml. A change here that is not
        // mirrored in the justfile would have this program never reading the installed default.
        let paths = config_paths();
        assert_eq!(
            paths.last().map(PathBuf::as_path),
            Some(Path::new("/etc/wlrix/lock.toml"))
        );
        assert!(
            paths
                .first()
                .is_none_or(|path| path.ends_with("wlrix/lock.toml"))
        );
    }
}
