// SPDX-License-Identifier: GPL-3.0-or-later
//! The wlRIX screen locker.
//!
//! Locks the session with `ext-session-lock-v1` and shows the time, the date, the account's
//! picture and name and a password field over the same wallpaper options `wlrix-bg` takes. Exits
//! 0 once the right password has unlocked the session, and non-zero if the session could not be
//! locked at all -- which `wlrix-idle` reports, because a locker that exits at once leaves the
//! session open and nothing else would ever say so.
//!
//! Started by `wlrix-idle` (`[lock] command = "wlrix-lock"`), or by hand.

use std::path::PathBuf;

fn usage() -> String {
    format!(
        "wlrix-lock {}\n\n\
         Locks the session. Needs a compositor with ext-session-lock-v1.\n\n\
         Usage: wlrix-lock [options]\n\n\
         Options:\n  \
           --config <path>        use this file instead of ~/.config/wlrix/lock.toml\n  \
           --check-config <path>  say whether that file would be accepted, exit\n  \
           -h, --help             this message\n  \
           -V, --version          print the version\n\n\
         Settings live in ~/.config/wlrix/lock.toml:\n\n  \
           blur = 0                     # blur radius over the wallpaper\n\n  \
           [appearance]\n  \
           palette = \"classic\"\n\n  \
           [clock]\n  \
           time_format = \"%H:%M\"\n  \
           date_format = \"\"             # empty: the locale's long date\n\n  \
           [background]                 # exactly background.toml's keys and defaults\n  \
           image = \"/path/to/picture.png\"\n  \
           mode  = \"fill\"               # tile, center, fit, fill, stretch, solid\n  \
           color = \"#555555\"\n\n\
         Per-monitor overrides go in [[background.output]] sections keyed by connector name.",
        env!("CARGO_PKG_VERSION")
    )
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut config_path: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                println!("{}", usage());
                return;
            }
            "--version" | "-V" => {
                println!("wlrix-lock {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            // Answers a question about a file rather than locking anything, so it needs no
            // compositor. The settings daemon and `wlrix-epoch`'s `check-config` run this.
            "--check-config" => {
                let Some(path) = args.get(i + 1) else {
                    eprintln!("wlrix-lock: --check-config needs a path");
                    std::process::exit(2);
                };
                if let Err(why) = wlrix_lock::config::check(std::path::Path::new(path)) {
                    eprintln!("wlrix-lock: {why}");
                    std::process::exit(1);
                }
                return;
            }
            "--config" => {
                let Some(path) = args.get(i + 1) else {
                    eprintln!("wlrix-lock: --config needs a path");
                    std::process::exit(2);
                };
                config_path = Some(PathBuf::from(path));
                i += 1;
            }
            other => {
                eprintln!("wlrix-lock: unknown argument: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    // Before any thread exists: `setlocale` is not thread-safe, and the PAM worker is a thread.
    wlrix_lock::clock::init_locale();
    ignore_stop_signals();

    let config = wlrix_lock::config::Config::load(config_path.as_deref());
    if let Err(err) = wlrix_lock::ui::run(config) {
        eprintln!("wlrix-lock: {err}");
        std::process::exit(1);
    }
}

/// Ignore the signals that would end a locker by accident.
///
/// Killing this process does not unlock anything -- the compositor keeps the session locked and
/// shows black -- but it does leave the person at the keyboard with no way to type a password
/// until something starts a new locker. A stray `SIGHUP` from a closed terminal, a `Ctrl+C` in
/// the one it was started from, or a session manager's `SIGTERM` should not be able to do that.
/// `SIGKILL` still works, for whoever really means it.
fn ignore_stop_signals() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
        // SAFETY: setting a disposition to SIG_IGN has no handler to be unsafe in.
        unsafe { libc::signal(signal, libc::SIG_IGN) };
    }
}
