# atuin-web-manage

A tiny local web UI to browse, search, edit, and bulk-delete entries in your
[atuin](https://github.com/atuinsh/atuin) shell-history SQLite database.

Because the atuin CLI is great for finding commands, but not for cleaning up
the noise: half-typed one-liners, secrets you accidentally pasted, huge
multi-line blobs that clutter `Ctrl+R`.

## Features

- Paginated history list, newest first
- Substring search on the `command` column
- Filter to "multi-line only" (great for pruning those annoying pasted blobs)
- Checkbox bulk delete
- Inline command edit
- Stats: total rows, unique commands, duplicates, date range

Deleted / edited rows take effect immediately in `Ctrl+R` — atuin re-reads the
DB on every search.

> **Warning:** this project assumes that atuin sync is disabled and history is
> stored in plaintext (the default when you never set up sync). Deletes are
> hard deletes on the local SQLite file; if you enable sync later, other
> machines can re-hydrate rows you thought were gone.
>
> Verify plaintext storage with:
>
> ```
> sqlite3 ~/.local/share/atuin/history.db "SELECT command FROM history LIMIT 3"
> ```
>
> If you see readable commands, you're good.

## Install

### Option 1 — Homebrew (macOS / Linux)

```
brew install ctxinf/tap/atuin-web-manage
```

### Option 2 — Clone and build

Requires a Rust toolchain (stable, 2021 edition).

```
git clone https://github.com/ctxinf/atuin-web-manage
cd atuin-web-manage
cargo build --release
# binary lands at target/release/atuin-web-manage
install -Dm755 target/release/atuin-web-manage ~/.local/bin/atuin-web-manage
```

## Usage

```
atuin-web-manage                             # bind to 127.0.0.1:8787
atuin-web-manage --bind 127.0.0.1:9000       # custom port
atuin-web-manage --bind 0.0.0.0:8787         # LAN-reachable (see warning below)
atuin-web-manage --db /path/to/history.db    # non-default DB path
```

Then open <http://127.0.0.1:8787>.

**Security note:** the server has no auth. Anyone who can reach the bind
address can read and delete your shell history. Keep it on `127.0.0.1` unless
you know what you're doing. A warning is printed to stderr if you bind to a
non-loopback address.

## Run as a systemd user service

Recommended way to keep it always available. Runs as your user, so it has
access to your `~/.local/share/atuin/history.db` without any privilege
escalation.

Save this to `~/.config/systemd/user/atuin-web-manage.service`:

```ini
[Unit]
Description=atuin-web-manage (local shell history UI)
After=default.target

[Service]
Type=simple
ExecStart=%h/.local/bin/atuin-web-manage --bind 127.0.0.1:8787
Restart=on-failure
RestartSec=2

# Hardening (optional but sensible for a local-only web server)
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=%h/.local/share/atuin
PrivateTmp=true

[Install]
WantedBy=default.target
```

Then enable and start it:

```
systemctl --user daemon-reload
systemctl --user enable --now atuin-web-manage
systemctl --user status atuin-web-manage
```

Logs:

```
journalctl --user -u atuin-web-manage -f
```

If you want the service to keep running after you log out (no active
graphical/tty session), enable lingering once:

```
sudo loginctl enable-linger $USER
```

## Releasing (maintainers)

The release pipeline uses [cargo-dist](https://github.com/axodotdev/cargo-dist).
Configuration lives in `[workspace.metadata.dist]` in `Cargo.toml`; the
generated workflow lives in `.github/workflows/release.yml`.

Pushing a semver tag triggers a full release:

```
# bump version in Cargo.toml, commit, then:
git tag v0.1.0
git push origin v0.1.0
```

The workflow will build binaries for x86_64/aarch64 Linux and macOS, publish a
GitHub Release with archives + checksums, and open a commit on
[`ctxinf/homebrew-tap`](https://github.com/ctxinf/homebrew-tap) updating the
formula.

Required repo secret: `HOMEBREW_TAP_TOKEN` — a fine-grained PAT with `contents:write`
on the `ctxinf/homebrew-tap` repo.

To regenerate the workflow after changing dist config:

```
cargo install cargo-dist --version 0.31.0
dist init
```

## License

MIT OR Apache-2.0
