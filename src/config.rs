// SPDX-License-Identifier: GPL-3.0-or-later
//! The hand-edited settings file.
//!
//! ```toml
//! # ~/.config/wlrix/lock.toml
//! blur = 0                    # blur radius over the wallpaper, in pixels; 0 is none
//!
//! [appearance]
//! palette = "classic"         # the color scheme of the field and button
//!
//! [clock]
//! time_format = "%H:%M"       # strftime, in the session's LC_TIME
//! date_format = ""            # empty: the locale's long date
//!
//! # Exactly the keys of background.toml, with exactly its defaults -- paste one in.
//! [background]
//! image = "/usr/share/wlrix/wallpapers/scatter.png"
//! mode  = "tile"
//! color = "#555555"
//!
//! [[background.output]]
//! name  = "DP-2"
//! image = "/home/you/Pictures/yaeka.png"
//! ```
//!
//! `[background]` is not a copy of `wlrix-bg`'s schema. It *is* `wlrix_bg::config::Config`,
//! deserialized by that crate's own types, so a key the desktop background accepts is a key the
//! lock screen accepts, means the same thing and defaults the same way -- and a key added there
//! arrives here with the next bump of the pin, without anyone remembering to.
//!
//! Read from the user's config directory first, then `/etc/wlrix`; the first file found wins
//! outright rather than merging. Unknown keys are an error, as everywhere in wlRIX.
//!
//! **A broken file never stops the lock.** Every other component costs the user a setting when
//! its config is wrong; this one would cost them the lock itself, which is the one outcome a
//! locker must not have. So a file that fails to parse is reported and the built-in defaults are
//! used, and the session locks regardless.

use std::path::Path;

use serde::Deserialize;

/// The largest blur radius honored, in pixels.
///
/// Past this the wallpaper is a smear of its average color either way, and each pass of the blur
/// is linear in the radius only because of the running sum -- a mistyped `blur = 5000` should not
/// be the reason the lock screen takes a second to appear.
pub const MAX_BLUR: u32 = 200;

/// What an unset `time_format` means: 24-hour hours and minutes, as in the reference design.
pub const DEFAULT_TIME_FORMAT: &str = "%H:%M";

/// `lock.toml`, as written.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The blur radius over the wallpaper, in pixels at the output's scale 1. Zero, the default,
    /// leaves the wallpaper exactly as `wlrix-bg` draws it.
    #[serde(default)]
    pub blur: u32,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub clock: ClockConfig,
    /// The wallpaper, in `background.toml`'s schema. Absent is the same as an empty
    /// `background.toml`: `fill`, `#555555`, no picture.
    #[serde(default)]
    pub background: wlrix_bg::config::Config,
}

/// Which color scheme the field and button are drawn in.
///
/// The section name and the key are the desktop's and the compositor's, so the files read alike.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceConfig {
    /// A scheme id from `wlrix-ui`: `classic`, `classic-g10`, `classic-g24`, `gotham`. Absent,
    /// empty or unrecognized is the default, with a line in the log for the last of those.
    #[serde(default)]
    pub palette: Option<String>,
}

/// How the clock and the date are written.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockConfig {
    /// A `strftime` format for the large clock. Absent or empty is [`DEFAULT_TIME_FORMAT`].
    #[serde(default)]
    pub time_format: Option<String>,
    /// A `strftime` format for the date under it. Absent or empty is the locale's long date;
    /// see [`crate::clock::long_date_format`].
    #[serde(default)]
    pub date_format: Option<String>,
}

impl Config {
    /// Load `path` if given, otherwise the first config file that exists.
    ///
    /// Never fails; see the module docs. A missing file is the ordinary case and says nothing.
    pub fn load(path: Option<&Path>) -> Self {
        let candidates = match path {
            Some(path) => vec![path.to_path_buf()],
            None => crate::xdg::config_paths(),
        };
        for path in candidates {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => {
                    eprintln!("wlrix-lock: could not read {}: {err}", path.display());
                    continue;
                }
            };
            return match toml::from_str::<Self>(&text) {
                Ok(config) => config.clamped(),
                Err(err) => {
                    eprintln!(
                        "wlrix-lock: {} is not valid, locking with the defaults: {err}",
                        path.display()
                    );
                    Self::default()
                }
            };
        }
        Self::default()
    }

    /// The config with out-of-range numbers pulled back into range, said once.
    fn clamped(mut self) -> Self {
        if self.blur > MAX_BLUR {
            eprintln!(
                "wlrix-lock: blur = {} is more than {MAX_BLUR}; using {MAX_BLUR}",
                self.blur
            );
            self.blur = MAX_BLUR;
        }
        self
    }

    /// The clock's format, with the default applied.
    pub fn time_format(&self) -> &str {
        match self.clock.time_format.as_deref() {
            None | Some("") => DEFAULT_TIME_FORMAT,
            Some(format) => format,
        }
    }

    /// The date's format, or `None` for the locale's long date.
    pub fn date_format(&self) -> Option<&str> {
        self.clock.date_format.as_deref().filter(|f| !f.is_empty())
    }
}

/// Parse a candidate config file, for `--check-config`.
///
/// Deliberately not [`Config::load`], which falls back to defaults: here the question *is*
/// whether the file is acceptable.
pub fn check(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?;
    toml::from_str::<Config>(&text)
        .map(|_| ())
        .map_err(|err| format!("{}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wlrix_bg::config::{DEFAULT_COLOR, DEFAULT_MODE, Mode};

    #[test]
    fn an_empty_file_is_what_an_empty_background_toml_is() {
        // The whole promise: same options, same defaults. An empty lock.toml must resolve to
        // what an empty background.toml does in wlrix-bg itself.
        let config: Config = toml::from_str("").expect("empty config should parse");
        let ours = config.background.resolve(None);
        let theirs = wlrix_bg::config::Config::default().resolve(None);
        assert_eq!(ours, theirs);
        assert_eq!(ours.mode, DEFAULT_MODE);
        assert_eq!(ours.color, DEFAULT_COLOR);
        assert_eq!(ours.image, None);
        assert_eq!(config.blur, 0, "no blur unless asked for");
        assert_eq!(config.time_format(), "%H:%M");
        assert_eq!(config.date_format(), None);
    }

    #[test]
    fn a_background_toml_pastes_in_verbatim() {
        let body = "image = \"/x/scatter.png\"\nmode = \"tile\"\ncolor = \"#123\"\n\n\
                    [[output]]\nname = \"DP-2\"\nmode = \"fit\"\n";
        // Under [background], every table header gains the prefix and nothing else changes.
        let lock = format!(
            "[background]\n{}",
            body.replace("[[output]]", "[[background.output]]")
        );
        let config: Config = toml::from_str(&lock).expect("should parse");
        let direct: wlrix_bg::config::Config = toml::from_str(body).expect("should parse");
        assert_eq!(
            config.background.resolve(Some("DP-2")),
            direct.resolve(Some("DP-2"))
        );
        assert_eq!(config.background.resolve(None).mode, Mode::Tile);
        assert_eq!(config.background.resolve(Some("DP-2")).mode, Mode::Fit);
    }

    #[test]
    fn a_typo_is_refused_rather_than_ignored() {
        assert!(toml::from_str::<Config>("blurr = 4\n").is_err());
        assert!(toml::from_str::<Config>("[background]\ncolour = \"#123456\"\n").is_err());
        assert!(toml::from_str::<Config>("[clock]\nformat = \"%H\"\n").is_err());
        assert!(toml::from_str::<Config>("[appearance]\nscheme = \"gotham\"\n").is_err());
    }

    #[test]
    fn an_absurd_blur_is_clamped() {
        let config: Config = toml::from_str("blur = 5000\n").expect("should parse");
        assert_eq!(config.clamped().blur, MAX_BLUR);
    }

    #[test]
    fn empty_formats_mean_the_defaults() {
        let config: Config = toml::from_str("[clock]\ntime_format = \"\"\ndate_format = \"\"\n")
            .expect("should parse");
        assert_eq!(config.time_format(), DEFAULT_TIME_FORMAT);
        assert_eq!(config.date_format(), None);
    }

    #[test]
    fn the_installed_default_config_is_one_this_program_accepts() {
        // `data/lock.toml.in` is what `just install` writes to /etc/wlrix/lock.toml. A typo in
        // it would not stop the lock -- see the module docs -- but it would silently cost every
        // user on the machine the scatter, which is what wlrix-bg's default shows.
        let template = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("data/lock.toml.in"),
        )
        .expect("data/lock.toml.in should be readable");
        let installed = template.replace("@WALLPAPERDIR@", "/usr/share/wlrix/wallpapers");
        assert!(
            !installed.contains('@'),
            "an unsubstituted @TOKEN@ is left in lock.toml.in"
        );

        let config: Config = toml::from_str(&installed).expect("the installed default must parse");
        let wallpaper = config.background.resolve(None);
        assert_eq!(
            wallpaper.image.as_deref(),
            Some(Path::new("/usr/share/wlrix/wallpapers/scatter.png"))
        );
        assert_eq!(wallpaper.mode, Mode::Tile);
        assert_eq!(wallpaper.color, DEFAULT_COLOR);
        assert_eq!(config.blur, 0);
    }

    #[test]
    fn the_installed_defaults_match_wlrix_bgs() {
        // The two templates must describe the same wallpaper, or a fresh machine locks to a
        // different picture than its desktop shows. wlrix-bg's template is not in this repo, so
        // the expectation is written out: the scatter, tiled, over the desktop gray.
        let template = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("data/lock.toml.in"),
        )
        .expect("data/lock.toml.in should be readable");
        assert!(template.contains("image = \"@WALLPAPERDIR@/scatter.png\""));
        assert!(template.contains("mode = \"tile\""));
        assert!(template.contains("color = \"#555555\""));
    }
}
