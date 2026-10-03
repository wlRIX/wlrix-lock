// SPDX-License-Identifier: GPL-3.0-or-later
//! The time and the date, as the lock screen writes them.
//!
//! Through the C library's `strftime` in the session's `LC_TIME`, which is what names the
//! weekday and the month in the user's language -- the reference design shows
//! "2026年9月18日金曜日" on a Japanese session, and that comes from the locale, not from a table
//! here. No date crate is in the tree, and the one thing this needs from one is exactly the part
//! the C library already does with the system's own locale data.
//!
//! What the C library does *not* have is a "long date" format. `%x` is the short one -- `09/18/26`
//! in `en_US` -- and nothing in POSIX gives the weekday-and-month-name form a lock screen shows.
//! So [`long_date_format`] supplies the order per language and leaves the names to `strftime`.

use std::ffi::{CStr, CString};

/// Take `LC_TIME` from the environment. Called once, at startup, before any thread exists --
/// `setlocale` is not thread-safe.
pub fn init_locale() {
    // SAFETY: a static NUL-terminated string; no other thread is running yet.
    unsafe { libc::setlocale(libc::LC_TIME, c"".as_ptr()) };
}

/// The name of the `LC_TIME` locale in effect, e.g. `ja_JP.UTF-8`.
fn current_locale() -> String {
    // SAFETY: a null locale queries without changing anything; the returned string is owned by
    // the C library and copied out before anything else can call setlocale.
    let name = unsafe { libc::setlocale(libc::LC_TIME, std::ptr::null()) };
    if name.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned()
}

/// The broken-down local time right now.
pub fn now() -> libc::tm {
    // SAFETY: `time` with a null argument only returns; `localtime_r` writes into our `tm`.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    }
}

/// Seconds until the minute turns over, so the clock can be redrawn exactly then rather than
/// polled. At least one, so a timer never fires twice inside the same second.
pub fn secs_to_next_minute(tm: &libc::tm) -> u64 {
    (60 - i64::from(tm.tm_sec).clamp(0, 59)) as u64
}

/// `tm` written with `format`.
///
/// An empty string for a format that cannot be passed to C (an interior NUL) or that produces
/// nothing at all; the clock is then blank rather than wrong.
pub fn format(tm: &libc::tm, format: &str) -> String {
    let Ok(format) = CString::new(format) else {
        return String::new();
    };
    let mut buffer = vec![0u8; 128];
    // `strftime` returns 0 both for "did not fit" and for a legitimately empty result, so grow a
    // few times and then accept the empty answer.
    while buffer.len() <= 4096 {
        // SAFETY: the buffer's true length is passed, and both pointers are live.
        let written = unsafe {
            libc::strftime(
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                format.as_ptr(),
                tm,
            )
        };
        if written > 0 {
            return String::from_utf8_lossy(&buffer[..written]).into_owned();
        }
        buffer.resize(buffer.len() * 2, 0);
    }
    String::new()
}

/// The long-date format for the session's `LC_TIME`.
pub fn long_date_format() -> &'static str {
    long_date_format_for(&current_locale())
}

/// The long-date format for a locale name, by language (and, for English, territory).
///
/// Only the *order* is decided here; the weekday and month names come from `strftime` in that
/// same locale. A language not listed gets the weekday and the locale's own short date, which is
/// never in the wrong order, merely terser.
fn long_date_format_for(locale: &str) -> &'static str {
    // `ja_JP.UTF-8@euro` -> `ja`, `JP`.
    let base = locale.split(['.', '@']).next().unwrap_or("");
    let mut parts = base.split('_');
    let language = parts.next().unwrap_or("");
    let territory = parts.next().unwrap_or("");
    match language {
        // "C" and "POSIX" are the untranslated English of the C library.
        "" | "C" | "POSIX" => "%A, %B %-d, %Y",
        "en" => match territory {
            "US" | "PH" | "" => "%A, %B %-d, %Y",
            _ => "%A %-d %B %Y",
        },
        // Year, month, day, then the weekday, with the units the locale's own `%x` uses.
        "ja" | "zh" => "%Y年%-m月%-d日%A",
        "ko" => "%Y년 %-m월 %-d일 %A",
        "de" | "da" | "nb" | "nn" | "no" | "fi" | "cs" | "sk" => "%A, %-d. %B %Y",
        "fr" | "it" | "es" | "pt" | "ca" | "ro" | "nl" | "sv" | "pl" | "ru" | "uk" | "el"
        | "tr" => "%A %-d %B %Y",
        "hu" => "%Y. %B %-d., %A",
        _ => "%A, %x",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Friday, September 18th 2026, 14:20:07 -- the moment in the reference picture.
    fn reference() -> libc::tm {
        // SAFETY: all-zero is a valid `tm`, and every field the formats read is set below.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        tm.tm_year = 2026 - 1900;
        tm.tm_mon = 8;
        tm.tm_mday = 18;
        tm.tm_wday = 5;
        tm.tm_yday = 260;
        tm.tm_hour = 14;
        tm.tm_min = 20;
        tm.tm_sec = 7;
        tm
    }

    #[test]
    fn the_clock_is_hours_and_minutes() {
        // The test process never calls `init_locale`, so this is the C locale.
        assert_eq!(format(&reference(), "%H:%M"), "14:20");
    }

    #[test]
    fn the_c_locale_long_date_reads_as_english() {
        assert_eq!(
            format(&reference(), long_date_format_for("C")),
            "Friday, September 18, 2026"
        );
    }

    #[test]
    fn the_japanese_order_matches_the_reference_design() {
        // The names would be 金曜日 in a ja_JP locale; in the C locale only the order shows.
        assert_eq!(
            format(&reference(), long_date_format_for("ja_JP.UTF-8")),
            "2026年9月18日Friday"
        );
    }

    #[test]
    fn locale_names_are_read_by_language() {
        assert_eq!(long_date_format_for("en_US.UTF-8"), "%A, %B %-d, %Y");
        assert_eq!(long_date_format_for("en_GB.UTF-8"), "%A %-d %B %Y");
        assert_eq!(long_date_format_for("de_DE.UTF-8@euro"), "%A, %-d. %B %Y");
        assert_eq!(long_date_format_for("xx_YY"), "%A, %x");
    }

    #[test]
    fn a_format_c_cannot_take_is_blank_rather_than_a_crash() {
        assert_eq!(format(&reference(), "%H\0%M"), "");
        assert_eq!(format(&reference(), ""), "");
    }

    #[test]
    fn the_tick_lands_on_the_minute() {
        let mut tm = reference();
        assert_eq!(secs_to_next_minute(&tm), 53);
        tm.tm_sec = 0;
        assert_eq!(secs_to_next_minute(&tm), 60);
        // A leap second is still at least one second away from firing twice.
        tm.tm_sec = 60;
        assert_eq!(secs_to_next_minute(&tm), 1);
    }
}
