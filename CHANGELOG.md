# Changelog

Notable changes are recorded here. Release tags use `vMAJOR.MINOR.PATCH` and match
the package version in `Cargo.toml`.

## Unreleased

- Implement native cloud discovery and immutable paged canonical history across clients.
- Continue, subscribe, and interrupt the original session with UUID correlation and monotonic durable sequences.
- Reject incomplete catch-up and oversized events explicitly; whitelist metadata and streamed envelopes.
- Local contract fixtures pass. Real-service qualification remains blocked by noninteractive authentication and is required before a qualified release.

## [Unreleased]

### Added

- Remote Claude Code session listing and official `claude --cloud` UI launcher.
- Version-1 machine broker for session control, bounded uploads, events, and interruption.
- OAuth credential isolation, private file fallback, and noninteractive macOS Keychain access.
- Offline contract tests and an opt-in live binary upload/readback probe.
- Public documentation, MIT license, security policy, package metadata, and CI.
- Manually gated Apple Silicon and Intel releases with checksums and GitHub build attestations.
- Companion-project links to rcodex and a shared maintenance checklist.

### Qualification

- Live session listing and metadata reads worked against the observed account.
- Isolated session creation returned HTTP 400; live upload/readback remains unqualified.

[Unreleased]: https://github.com/Typiqally/rclaude/commits/main
