# rclaude — remote control for Claude Code

`rclaude` is an unofficial Rust command-line controller for remote Claude Code
sessions. It opens the installed official `claude --cloud` terminal UI and
provides a machine-only broker for session control, bounded file uploads, and
event streaming for machine integrations.

The companion project [rcodex](https://github.com/Typiqally/rcodex) connects the
official OpenAI Codex terminal UI to a paired remote Codex host. Both projects
follow the same publishing and security conventions, with independent provider
protocols and release versions. See
[related-project maintenance](docs/related-projects.md).

```text
rclaude connect ───── official claude --cloud terminal UI ───── Claude session
rclaude broker ────── Anthropic session / Files APIs ────────── Claude session
```

## What rclaude provides

- Session listing and connection to an existing Claude Code cloud session.
- The official Claude terminal interface for interactive conversations and approvals.
- File staging before opening that interface, with remote mount paths printed locally.
- A versioned JSON-lines broker for native discovery, complete snapshot history, session open/create, uploads, turns,
  interruption, and controller events.
- A Rust controller with no Node runtime dependency of its own.

> [!WARNING]
> `rclaude` is unofficial experimental software, not affiliated with or supported
> by Anthropic. Its controller integration relies on observed upstream interfaces
> that can change without notice. Session listing and metadata reads worked live;
> isolated session creation returned HTTP 400. **Live upload and remote binary
> readback have not passed.** Do not treat attachment support as qualified.

## Current compatibility

- Rust 1.95 or newer is required to build.
- macOS is the current CI and binary-release platform; Linux credential-file
  fallback exists in the source but is not yet live-qualified.
- The official Claude CLI must already be installed, signed in with Claude OAuth,
  and expose `--cloud`. API-key-only credentials are not supported.
- Remote sessions and account access must be available through the official service.
- The observed baseline is Claude CLI `2.1.295`; this is evidence, not a guarantee
  that every command or account is supported.

No device-attestation or enrollment checks are bypassed. Device-binding refusals
require the official enrollment flow. See
[protocol observations](docs/protocol-observations.md) for interfaces and limits.

## Install

Install Rust and the official Claude CLI, then sign in through that CLI. From
this repository:

```sh
cargo install --locked --path . --bin rclaude
rclaude --help
rclaude auth-status
```

Once the repository is public, you can also install directly from source:

```sh
cargo install --locked --git https://github.com/Typiqally/rclaude --bin rclaude rclaude
```

Source installation is the initial distribution method. The
[release workflow](RELEASING.md) can publish attested Apple Silicon and Intel
macOS binaries after personal and live qualification. There is no built-in updater.

For explicit local macOS Keychain authorization, build and install with a local
signing certificate, then request access explicitly:

```sh
python3 scripts/install_local_macos.py
/opt/homebrew/bin/rclaude authorize
```

macOS may ask for the signing key and the official Claude credential. Choose
**Always Allow** to remember access. Enter passwords only in the macOS dialog.
The installer retains a nonexportable signing key in the login Keychain. It
installs a certificate-signed launcher at `/opt/homebrew/bin/rclaude` and a
separately signed engine at `/opt/homebrew/bin/.rclaude/engine.dylib`. Engine
updates preserve the launcher bytes and its remembered credential grant. The
launcher verifies a private copy against its pinned certificate and engine
identifier before loading it into the same process. Credentials never cross a
launcher/engine IPC channel. See [ADR-0001](docs/adr/0001-signed-engine-launcher.md).

This local certificate is for development, not notarization. macOS assigns
self-signed executables a [hash-based Keychain partition](https://github.com/apple-oss-distributions/Security/blob/main/securityd/src/clientid.cpp).
Changing the launcher or its certificate requires explicit authorization again;
ordinary engine updates do not. Normal broker calls never open a credential
dialog. Installing a monolithic binary with `cargo install` replaces the stable
launcher and does not preserve its grant. Use the Python installer for local
macOS updates; Python is build tooling only and is not required by the broker.

## Use

```sh
rclaude sessions
rclaude connect --session cse_EXAMPLE
rclaude connect --session https://claude.ai/code/cse_EXAMPLE --file ./notes.txt
rclaude upload --session cse_EXAMPLE --file ./notes.txt
```

`connect --file` stages files into the existing session and prints their remote
mount paths before opening the official UI. Reference those paths in your
message. Repeat `--file` for multiple files. Limits are 10 files, 10 MiB per file,
and 25 MiB total.

`rclaude broker` uses strict version-1 JSON lines, 64 KiB maximum frames, and
contiguous 32 KiB base64 upload chunks. It is intended for machine integrations;
interactive remote approvals stay in the official Claude UI.

## Security and data

- Provider credentials stay inside the controller and the official credential store;
  they are never returned through the broker.
- macOS Keychain reads skip items requiring authentication. Reads and refresh
  writes disable interactive Keychain UI; denied access produces a typed error.
- Only the explicit `authorize` CLI can request a macOS access grant. Broker
  frames cannot invoke it; authorization does not export or print credentials.
- Usable OAuth credentials are cached in memory. Refresh can update the official
  credential record while preserving unrelated fields.
- `CLAUDE_CONFIG_DIR` and `CLAUDE_SECURESTORAGE_CONFIG_DIR` follow the official
  credential namespace. Fallback `.credentials.json` files must be private
  (0600 on Unix).
- HTTP redirects are disabled. Upstream error bodies stay private and become
  typed error codes.
- There is no telemetry or managed relay operated by this project.

Report vulnerabilities according to [SECURITY.md](SECURITY.md). Never commit
credentials, real session identifiers, private transcripts, or qualification logs.

## Development and qualification

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
python3 -m unittest discover -s tests -p 'test_*.py'
cargo audit
cargo package --locked --allow-dirty
```

Offline tests cover credential redaction, noninteractive Keychain guards, bounded
protocol input, upload chunks, fragmented worker SSE events, and HTTP multipart
upload → session staging → temporary Files API deletion. Integration consumers
should provide their own fake-broker fixtures. Offline tests do not establish live
account support.

The manual probe below creates a separate remote session, uploads a binary, and
requests a SHA-256 readback. Run it only when deliberately qualifying your account
and protocol; it uses the real service and may consume account usage. Keep its
output private and clean up the session through the official UI afterwards.

```sh
cargo run --locked --example qualify -- cse_PARENT_EXAMPLE
```

See [CONTRIBUTING.md](CONTRIBUTING.md) and [RELEASING.md](RELEASING.md).

## Uninstall

```sh
cargo uninstall rclaude
```

`rclaude` uses the official Claude login rather than creating a separate
controller identity. Manage sign-out and remote sessions through the official
Claude tools; uninstalling this binary does not revoke that login.

## License and trademarks

Available under the [MIT License](LICENSE). Anthropic, Claude, and Claude Code
are trademarks of their respective owners. Their use describes compatibility
and does not imply endorsement. No official application source or assets are
redistributed.

Native Claude cloud conversations use the [version 1 broker contract](docs/broker-contract.md), including bounded discovery, immutable paged history, original-session continuation, and interruption. Local contract tests pass. Live discovery now returns 108 sessions, and authorization survives engine updates. Live history paging works for representable events; individual events exceeding the 64 KiB frame limit fail explicitly with `too_large`. Complete all-client history, catch-up truncation, continuation, interruption, and uploads still require qualification. This support is experimental.

If `auth-status` reports `credential_access_denied`, the official credential record
is present but macOS has denied this broker access. Install with the local signing
installer and run `rclaude authorize` explicitly to request a remembered grant.
Normal operations keep Keychain interaction disabled. Repeated broker retries do
not request access. Do not export tokens into Bluedex or change Keychain ACLs
automatically.
