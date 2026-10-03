// SPDX-License-Identifier: GPL-3.0-or-later
//! Who is locked out: the name to show, the account PAM checks, and where their picture is.
//!
//! Always the user running this process. A locker that could be told to authenticate somebody
//! else would be a password oracle with a nice background, so there is no option for it.

use std::ffi::CStr;
use std::path::{Path, PathBuf};

/// Where AccountsService keeps account pictures, one file per login name.
///
/// KDE's and GNOME's settings panels both write here, so this is where a picture chosen on
/// another desktop on the same machine already is.
const ACCOUNTS_SERVICE_ICONS: &str = "/var/lib/AccountsService/icons";

/// The account the session belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// The login name, which is what PAM is asked about.
    pub login: String,
    /// What the lock screen shows: the GECOS name, or the login name when there is none.
    pub display_name: String,
    /// The home directory, for `~/.face`.
    pub home: Option<PathBuf>,
}

impl User {
    /// The user running this process, from the password database.
    ///
    /// Through `getpwuid_r` rather than reading `/etc/passwd`, so an LDAP or systemd-homed
    /// account is found too -- the greeter can afford to list only local accounts, but a locker
    /// that could not find the person in front of it could not unlock for them.
    pub fn current() -> Result<Self, String> {
        // SAFETY: getuid cannot fail and has no preconditions.
        let uid = unsafe { libc::getuid() };
        let mut buffer = vec![0u8; 4096];
        loop {
            // SAFETY: `passwd` is plain old data, and all-zero is a valid (empty) value for it.
            let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
            let mut result: *mut libc::passwd = std::ptr::null_mut();
            // SAFETY: every pointer is to a live local, and `buffer.len()` is its true size.
            let status = unsafe {
                libc::getpwuid_r(
                    uid,
                    &mut entry,
                    buffer.as_mut_ptr().cast(),
                    buffer.len(),
                    &mut result,
                )
            };
            if status == libc::ERANGE && buffer.len() < 1 << 20 {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            if status != 0 || result.is_null() {
                return Err(format!("no password database entry for uid {uid}"));
            }
            // SAFETY: on success the strings point into `buffer`, which is still alive, and are
            // NUL-terminated.
            let field = |ptr: *const libc::c_char| -> String {
                if ptr.is_null() {
                    String::new()
                } else {
                    unsafe { CStr::from_ptr(ptr) }
                        .to_string_lossy()
                        .into_owned()
                }
            };
            let login = field(entry.pw_name);
            let gecos = field(entry.pw_gecos);
            let dir = field(entry.pw_dir);
            if login.is_empty() {
                return Err(format!("the account for uid {uid} has no name"));
            }
            let home = if dir.is_empty() {
                crate::xdg::home()
            } else {
                Some(PathBuf::from(dir))
            };
            return Ok(Self {
                display_name: display_name(&gecos, &login),
                login,
                home,
            });
        }
    }

    /// Where this user's picture might be, most specific first.
    ///
    /// `~/.face.icon` before `~/.face` because that is the order KDE reads them in, and KDE is
    /// the desktop that writes both. AccountsService last: it is the system's copy, and a user
    /// who has put a file in their own home meant that one.
    pub fn avatar_candidates(&self) -> Vec<PathBuf> {
        avatar_candidates(self.home.as_deref(), &self.login)
    }
}

/// See [`User::avatar_candidates`].
fn avatar_candidates(home: Option<&Path>, login: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home {
        paths.push(home.join(".face.icon"));
        paths.push(home.join(".face"));
    }
    // A login name with a slash in it cannot exist, but the path is built from it, so refuse
    // rather than trust that.
    if !login.contains('/') && login != "." && login != ".." {
        paths.push(Path::new(ACCOUNTS_SERVICE_ICONS).join(login));
    }
    paths
}

/// The name to show: GECOS's first comma-separated field, or the login name.
///
/// The rest of GECOS is the office and phone fields nobody fills in -- the greeter reads it the
/// same way.
fn display_name(gecos: &str, login: &str) -> String {
    let name = gecos.split(',').next().unwrap_or("").trim();
    if name.is_empty() {
        login.to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_display_name_is_the_first_gecos_field() {
        assert_eq!(
            display_name("Victor Fugazzotto,,,", "vic"),
            "Victor Fugazzotto"
        );
        assert_eq!(display_name("  Ada  ,Room 4", "ada"), "Ada");
    }

    #[test]
    fn an_empty_gecos_shows_the_login_name() {
        assert_eq!(display_name("", "vic"), "vic");
        assert_eq!(display_name(",,,", "vic"), "vic");
    }

    #[test]
    fn avatars_are_looked_for_in_kdes_order() {
        let paths = avatar_candidates(Some(Path::new("/home/vic")), "vic");
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/home/vic/.face.icon"),
                PathBuf::from("/home/vic/.face"),
                PathBuf::from("/var/lib/AccountsService/icons/vic"),
            ]
        );
    }

    #[test]
    fn a_login_name_cannot_walk_out_of_the_icon_directory() {
        assert!(
            avatar_candidates(None, "../../etc/shadow").is_empty(),
            "a slash in the name must not reach a path"
        );
        assert!(avatar_candidates(None, "..").is_empty());
    }

    #[test]
    fn the_current_user_is_found() {
        // Whoever runs the tests has an account; this is the call the locker cannot do without.
        let user = User::current().expect("the test runner has a passwd entry");
        assert!(!user.login.is_empty());
        assert!(!user.display_name.is_empty());
    }
}
