#!/usr/bin/env just --justfile
name := 'wlrix-lock'

rootdir := ''
prefix := '/usr'

# Which distribution's PAM stack to install. `arch` uses the system-auth include, `debian` the
# common-* ones; the two are not interchangeable, and installing the wrong one leaves a stack
# whose includes do not exist -- which is a lock screen that cannot be unlocked.
#
# Read from the environment rather than declared as a variable, for the reason
# wlrix-greeter's justfile gives: `wlrix-epoch` passes it down to every component, and `just`
# refuses an override for a variable a justfile does not declare.
pam-flavor := env("PAM_FLAVOR", "arch")

base-dir := absolute_path(clean(rootdir / prefix))
bin-dir := base-dir / 'bin'
# PAM and the config file go in /etc, which is not under the prefix wherever the prefix is.
etc-dir := absolute_path(clean(rootdir / 'etc'))

bin-src := 'target' / 'release' / name
bin-dst := bin-dir / name

pam-src := 'setup' / name + '.pam.' + pam-flavor
pam-dst := etc-dir / 'pam.d' / name

# The system default config: a template, because it names the installed wallpaper by absolute
# path. The same @WALLPAPERDIR@ arrangement as wlrix-bg's background.toml.in.
config-src := 'data' / 'lock.toml.in'
config-dst := etc-dir / 'wlrix' / 'lock.toml'

# Where `wlrix-assets` installs the wallpapers. **Hand-kept in step with that repo's justfile**,
# and with wlrix-bg's, which carries the same line.
wallpaper-dir := clean(prefix / 'share' / 'wlrix' / 'wallpapers')

default:
  @just --list

release:
  cargo build --release

lint:
  cargo clippy

test:
  cargo test

# Install the locker, its PAM stack and its system default config.
#
# Deliberately does not build: this is normally run as root, and building as root leaves a
# target directory nobody can write to afterwards.
#
#     just release && sudo just install
[doc("Install the locker, its PAM stack and default config (build first; run as root)")]
install:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -x '{{bin-src}}' ]; then
    echo "no release build -- run 'just release' first" >&2
    exit 1
  fi
  if [ ! -f '{{pam-src}}' ]; then
    echo "no PAM stack for PAM_FLAVOR={{pam-flavor}} (expected {{pam-src}})" >&2
    exit 1
  fi
  install -Dm0755 '{{bin-src}}' '{{bin-dst}}'
  echo "installed {{bin-dst}}"

  # The PAM stack is ours and is replaced every time, like the greeter's. 0644, as PAM wants:
  # root-owned and world-readable, never writable by anyone else. Without it, PAM falls back to
  # the `other` service, which denies everything on most distributions -- a lock screen that
  # can never be unlocked -- so it is not optional.
  install -Dm0644 '{{pam-src}}' '{{pam-dst}}'
  echo "installed {{pam-dst}} (PAM stack: {{pam-flavor}}; set PAM_FLAVOR= to change it)"

  # /etc/wlrix belongs to whoever installed the machine; an existing config is left alone.
  if [ -e '{{config-dst}}' ]; then
    echo "kept {{config-dst}} (already present; not overwritten)"
  else
    # DESTDIR must not leak into the path the file itself carries.
    install -d "$(dirname '{{config-dst}}')"
    sed 's|@WALLPAPERDIR@|{{wallpaper-dir}}|g' '{{config-src}}' > '{{config-dst}}'
    chmod 0644 '{{config-dst}}'
    echo "installed {{config-dst}}"
  fi
  echo
  echo "To lock on idle, set [lock] command = \"{{name}}\" in wlrix-idle's idle.toml."

# Remove what `install` put down.
#
# The PAM stack goes with the binary -- it serves nothing else. The config file is left behind
# on purpose, as wlrix-bg's uninstall leaves its own.
[doc("Remove what install put down")]
uninstall:
  #!/usr/bin/env bash
  set -euo pipefail
  rm -f '{{bin-dst}}' '{{pam-dst}}'
  echo "removed {{bin-dst}} {{pam-dst}}"
  if [ -e '{{config-dst}}' ]; then
    echo "left {{config-dst}} alone -- remove it by hand if you want it gone"
  fi

clean:
  cargo clean
