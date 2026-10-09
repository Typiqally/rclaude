# Contributing

Keep changes narrow, document observed upstream behavior, and add a regression
test for bug fixes. Use Rust 1.95 or newer; macOS is the currently tested platform.

## Local checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
python3 -m unittest discover -s tests -p 'test_*.py'
cargo audit
cargo package --locked --allow-dirty
```

Tests must use synthetic credentials and isolated loopback HTTP fixtures. The
Keychain guard test only checks that interaction is disabled; it must not read or
write a real credential item. Never run live session creation, uploads, token
refresh, or the qualification example in public CI.

## Pull requests

- Keep credentials, account IDs, session IDs, transcripts, and private host or
  repository information out of commits and logs. Use obvious example fixtures.
- Keep provider tokens out of IPC, command-line arguments, and diagnostics.
- Preserve bounds on frames, streams, files, concurrency, and upload lifetimes.
- Preserve unrelated fields in official credential files. Broker operations and
  `auth-status` must never enable interactive Keychain prompts. Only the explicit
  certificate-signed `rclaude authorize` CLI may request access through macOS;
  it is unavailable through broker frames. Do not bypass device enrollment.
- Record protocol assumptions in `docs/protocol-observations.md` and distinguish
  offline test coverage from live qualification.
- Update `CHANGELOG.md` for user-visible changes.

## Related project

For changes that also apply to [rcodex](https://github.com/Typiqally/rcodex), follow
the [related-project maintenance checklist](docs/related-projects.md). Record the
companion change or explain why it does not apply in the PR. Versions and
provider-specific protocol work remain independent.

By contributing, you agree that your contribution is licensed under the MIT License.
