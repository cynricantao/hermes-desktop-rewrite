# Installing and updating (per user)

`scripts/install-user.sh` installs the app for the current user and keeps it up to date with
this repository's `main` branch. Nothing runs as root.

```sh
scripts/install-user.sh               # install, build the first version, enable updates
scripts/install-user.sh --uninstall   # remove app, launcher and update units
```

## What gets installed

| Path | What |
|---|---|
| `~/.local/lib/hermes-desktop-rs/hermes-desktop-rs` | the app (`.prev` = the version before the last update) |
| `~/.local/lib/hermes-desktop-rs/hermes-browser` | the in-app browser helper (found next to the app) |
| `~/.local/lib/hermes-desktop-rs/hermes-desktop-rs-update` | the updater (`scripts/self-update.sh`, updates itself) |
| `~/.local/lib/hermes-desktop-rs/COMMIT` | the commit that is installed |
| `~/.local/share/applications/hermes-desktop-rs.desktop` | the "Hermes Desktop (Rust)" menu entry |
| `~/.local/share/hermes-desktop-rs/` | private clone + build cache (not your development checkout) |
| `~/.local/state/hermes-desktop-rs/update.log` | update log |
| `~/.config/systemd/user/hermes-desktop-rs-update.*` | the update units |

## When it updates

* **After every successful `hermes update`.** `hermes update` writes its exit code to
  `~/.hermes/.update_exit_code`; `hermes-desktop-rs-update.path` watches that file and the
  updater runs only when it reads `0`.
* **Once a day** (`hermes-desktop-rs-update.timer`, catches up after the machine was off),
  for changes merged to `main` between Hermes updates.

Each run fetches `main`; if it moved, it builds the app and the browser helper at low
priority, then swaps the binaries in atomically. A running app is not affected: the new
version is used the next time you open it, and a desktop notification says so. If the build
fails, the installed version stays and you get a notification with the log path.

Everything merged to `main` is built and installed automatically, so only merge what you
want to run.

## Useful commands

```sh
~/.local/lib/hermes-desktop-rs/hermes-desktop-rs-update            # update now if main moved
~/.local/lib/hermes-desktop-rs/hermes-desktop-rs-update --force    # rebuild and reinstall
systemctl --user list-timers hermes-desktop-rs-update.timer        # next daily check
journalctl --user -u hermes-desktop-rs-update -u hermes-desktop-rs-update-after-hermes
# Roll back to the previous version:
cd ~/.local/lib/hermes-desktop-rs && mv hermes-desktop-rs.prev hermes-desktop-rs
```

Requirements: Rust (`cargo`, via rustup), `git`, and for the browser helper `gtk3` and
`webkit2gtk-4.1`.
