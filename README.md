# atuin-web-manage

A tiny local web UI to browse, search, edit, and bulk-delete entries in your
[atuin](https://github.com/atuinsh/atuin) shell-history SQLite database.

Because the atuin CLI is great for finding commands, but not for cleaning up
the noise: half-typed one-liners, secrets you accidentally pasted, huge
multi-line blobs that clutter `Ctrl+R`.

## Features

Search, filter (incl. multi-line only), inline edit, and checkbox bulk delete
against your atuin history. Changes take effect in `Ctrl+R` immediately.

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
atuin-web-manage                             # bind to 127.0.0.1:28057
atuin-web-manage --bind 127.0.0.1:9000       # custom port
atuin-web-manage --bind 0.0.0.0:8787         # LAN-reachable (see warning below)
atuin-web-manage --db /path/to/history.db    # non-default DB path
```

Then open <http://127.0.0.1:28057>.

**Security note:** the server has no auth. Anyone who can reach the bind
address can read and delete your shell history. Keep it on `127.0.0.1` unless
you know what you're doing. A warning is printed to stderr if you bind to a
non-loopback address.

## Search

Use the compact `sort by: time | relevance` controls below the search box:

- **relevance** (default) matches Atuin **18.16.1** default interactive
  **GLOBAL / fuzzy / smart_sort=false** search: smart case, query operators,
  regex, user-author filtering, deduplication, and minspan ranking. Like the
  CLI, it ranks the newest **200 unique matching commands**, then paginates.
- **time** keeps every matching history record, including duplicates and agent
  records, ordered newest first for editing and deletion.

`A B C` matches `A yy B yxxx C lyy`. Operators include `'exact`, `^prefix`,
`suffix$`, `!exclude`, `|`, and `r/regex/`. SQL wildcards follow Atuin's behavior.
Invalid regexes return no results. The multi-line filter works in both views.

Compatibility is pinned to the version and default settings above; custom CLI
settings and daemon search modes are outside this compatibility target.
The adapted Atuin code is covered by [its MIT license](licenses/Atuin-MIT.txt).

Run `cargo test --locked` for regression tests. To compare a running service
against the installed Atuin 18.16.1 CLI, including result IDs, ordering and
pagination without printing history contents:

```sh
python3 scripts/check-cli-search.py
```

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
ExecStart=%h/.local/bin/atuin-web-manage --bind 127.0.0.1:28057
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
