# wlrix-lock

The wlRIX screen locker. Locks the session with `ext-session-lock-v1` and shows the time, the date, your picture and
name, and a password field, over the same wallpaper options `wlrix-bg` takes.

- **Language:** Rust
- **License:** GPL-3.0-or-later
- **Reference:** KDE Plasma's lock screen for the arrangement, IRIX's Motif chrome for the widgets

`wlrix-idle` had a lock leg and `wlrix-compositor` already implemented the lock protocol, failing closed; what was
missing was a locker of our own. This is it. It replaces `swaylock` in `idle.toml`.

## Using it

```toml
# ~/.config/wlrix/idle.toml
[[timeout]]
after_secs = 900
lock = true
blank = true

[before_sleep]
lock = true

[lock]
command = "wlrix-lock"
```

Or run `wlrix-lock` by hand. It exits 0 once the right password has unlocked the session, and 1 if the session could
not be locked at all -- most often because another locker already holds it, which the compositor answers by refusing.

## What it does, and does not

- **Locks before it draws.** The lock is requested first and the lock surfaces follow, one per output, including any
  monitor plugged in while locked. A refused lock exits at once: nothing that looks like a lock screen is ever drawn
  over an open session.
- **Unlocks only on PAM's say-so.** There is no other path to `unlock_and_destroy`.
- **Killing it does not unlock.** The compositor keeps the session locked and shows black. `SIGINT`, `SIGTERM`, `SIGHUP`
  and `SIGQUIT` are ignored so a stray one cannot leave you there. Whether a new `wlrix-lock` started from a TTY can
  take over a lock whose locker died is the compositor's call, and has not been tested yet.
- **The password is kept carefully**: in a buffer that clears what it leaves behind, never logged, never printed by a
  `Debug`, and handed to PAM through a hand-written conversation function rather than a wrapper's `String`.

Keys: type, `Backspace`, `Escape` or `Ctrl+U` to clear, `Enter` to unlock. Clicking the button unlocks too. A Caps
Lock warning appears under the field.

## Configuration

`~/.config/wlrix/lock.toml`, then `/etc/wlrix/lock.toml`. The first file found wins outright and nothing is merged, as
everywhere in wlRIX. Unknown keys are an error.

**A broken file never stops the lock.** It is reported on stderr and the built-in defaults are used. Every other
component pays for a typo with a setting; this one would pay with the lock itself.

```toml
blur = 0                    # blur radius over the wallpaper, in pixels; 0 is none

[appearance]
palette = "classic"         # classic | classic-g10 | classic-g24 | gotham

[clock]
time_format = "%H:%M"       # strftime, in your LC_TIME
date_format = ""            # empty: the locale's long date

# Exactly background.toml's keys, with exactly its defaults.
[background]
image = "/usr/share/wlrix/wallpapers/scatter.png"
mode = "tile"               # tile | center | fit | fill | stretch | solid
color = "#555555"

[[background.output]]
name = "DP-2"
image = "/home/you/Pictures/yaeka.png"
mode = "fit"
```

### `[background]` is wlrix-bg's

Not a copy of its schema: it *is* `wlrix_bg::config::Config`, decoded and painted by `wlrix-bg`'s own library. So the
modes, the color syntax, the SGI/XPM/XBM/PCX decoders, the size limits and the fallback to the color all behave exactly
as they do on the desktop, and an empty `[background]` is what an empty `background.toml` is: `fill` over `#555555`. A
`background.toml` pastes in under `[background]` unchanged, with `[[output]]` becoming `[[background.output]]`.

`just install` writes `/etc/wlrix/lock.toml` from [`data/lock.toml.in`](data/lock.toml.in) -- the IRIX scatter, tiled,
the same picture `wlrix-bg` installs -- and never overwrites one that is already there.

### Blur

Off by default, so the default lock screen shows the wallpaper exactly as the desktop does. A three-pass box blur,
within a few percent of a Gaussian, run once per output when the lock appears; about 0.3 s for a 4K output in a
release build.

### The date

`strftime` in your `LC_TIME`, so the weekday and month are in your language. POSIX has no "long date" format, so the
*order* is chosen per language (`2026年9月18日金曜日`, `Friday, September 18, 2026`, `Freitag, 18. September 2026`)
and anything not covered gets the weekday and the locale's own short date. `date_format` overrides it outright.

### The picture

`~/.face.icon`, then `~/.face`, then `/var/lib/AccountsService/icons/<login>` -- KDE's order, and the files KDE's and
GNOME's settings write. PNG or JPEG. Without one, a drawn head-and-shoulders stands in. The name is the account's GECOS
name, or the login name when there is none.

## PAM

The service is `wlrix-lock`, installed to `/etc/pam.d/wlrix-lock` from `setup/wlrix-lock.pam.$PAM_FLAVOR` (`arch`, the
default, or `debian`). Only `auth` and `account`: a locker re-proves who is at the keyboard and opens nothing. It runs
unprivileged; `pam_unix` checks the password through its setuid helper `unix_chkpwd`.

**Without that file the session cannot be unlocked** -- PAM falls back to the `other` service, which denies everything
on most distributions. That is why `install` refuses to proceed without a stack for the chosen flavor.

## Options

| Option                  | Meaning                                                     |
|-------------------------|-------------------------------------------------------------|
| `--config <path>`       | Use this config file instead of searching the usual places. |
| `--check-config <path>` | Say whether that file would be accepted, and exit.          |
| `-h`, `--help`          | Print usage and exit.                                       |
| `-V`, `--version`       | Print the version and exit.                                 |

## Looking at it without locking

```bash
cargo run --release --example paint_png -- out.png 1920 1080 1 --typed 6 --message "Unlocking failed" --caps
```

renders exactly what one output would show -- same config lookup, wallpaper, blur, layout and widgets -- to a PNG,
without a compositor and without a password to type to get the screen back.

## Building

```bash
cargo build --release
just release && sudo just install
```

Needs libpam and xkbcommon (`libpam0g-dev` and `libxkbcommon-dev` on Debian), and a compositor with
`ext-session-lock-v1`.

## What it does not do yet

- **Sleep and switch-user buttons**, which KDE shows under the field. There is nothing in wlRIX to switch users to yet.
- **Translations.** The few strings of its own (`Password`, `Unlocking failed`, `Caps Lock is on`) are English; the date
  is already localized by the C library.
- **Fractional scale.** Buffers are at the output's integer scale, as in `wlrix-bg`.
- **Reloading.** There is deliberately no `SIGHUP` reload: a lock screen should not change under the person typing into
  it. The next lock reads the file again.
