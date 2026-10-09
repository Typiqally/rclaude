# Releasing

Publish only the standalone `rclaude` repository and its intentional history.
Never publish the parent workspace or another project's files. The intended
destination is <https://github.com/Typiqally/rclaude>.

## First public source publication

1. Create the standalone GitHub repository and configure its `origin`. Review
   the working tree and all history intended for publication for credentials,
   account/session identifiers, private transcripts, and unrelated files.
2. Finish any in-progress feature work. Formatting, lint, and all tests must pass;
   do not omit unfinished tests to make a release pass.
3. Run the local checks in [CONTRIBUTING.md](CONTRIBUTING.md), inspect
   `cargo package --locked --allow-dirty --list`, and test installation from the
   generated package in a temporary directory.
4. Preserve experimental status and current qualification failures in the
   README, changelog, and protocol observations. Source publication does not
   establish qualified upload support.
5. Use `main` as the default branch. Commit and push the reviewed source and
   publishing files; require CI for subsequent changes.
6. Enable GitHub private vulnerability reporting so `SECURITY.md` has a working
   private report destination. GitHub Actions must be enabled for CI and releases.

`publish = false` intentionally disables crates.io publishing, as in
[rcodex](https://github.com/Typiqally/rcodex). GitHub source and binary releases
are the distribution channels while upstream behavior remains experimental.

## Signed binary releases

The local `scripts/install_local_macos.py` installer uses a reusable self-signed
certificate with an explicitly pinned designated requirement for development.
It does not change system trust or provide Developer ID signing or notarization.
Keep its private signing key local; never include it in source or release assets.
Offline installer tests must not create identities or request credential access.

The `Signed macOS release` workflow mirrors rcodex's manually dispatched release
model. It validates the tag/version, repeats offline checks and dependency audit,
verifies the source package, builds Apple Silicon and Intel binaries, creates
`SHA256SUMS`, and attests the binaries and checksum file through GitHub/Sigstore.
Attestations establish build provenance; they are not Apple signing or notarization.
This requires a public repository or a GitHub plan supporting private attestations.

Before each release:

1. Update `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, and observed compatibility
   evidence. Review [related-project maintenance](docs/related-projects.md).
2. Pass all local checks and inspect the complete packaged file list.
3. Personally test the exact revision on an authenticated Mac. Exercise session
   listing, metadata, official UI connection, interruption, and reconnect.
4. Qualify isolated session creation, a binary upload larger than 1 MiB, exact
   remote size/SHA-256 readback, and temporary Files API cleanup on success and
   failure. Check that denied credential access cannot open a Keychain dialog.
   The current live creation/upload failures block this sign-off.
5. Keep accounts, credentials, and private logs out of CI. Record sanitized CLI
   versions, platform, date, and passed/failed operations in protocol observations.
6. Commit the reviewed revision, create a signed matching version tag, and push it.

After personal and live checks pass, dispatch **at the tag**:

```sh
gh workflow run release.yml --ref vMAJOR.MINOR.PATCH \
  -f manual_test_confirmed=true -f live_cloud_validated=true
```

Dispatching from a branch, using a tag that does not match `Cargo.toml`, or leaving
either confirmation false fails before building. No real accounts are used by
the workflow; the inputs record the maintainer's qualification confirmation.

Publication creates a draft, uploads all assets, and then publishes it. If it
fails partway, inspect and remove the incomplete draft before rerunning. Never
overwrite assets of a published release; increment the package version instead.

## Verify downloaded binaries

Use GitHub CLI attestation verification for the selected binary and `SHA256SUMS`:

```sh
gh attestation verify ./rclaude-VERSION-aarch64-apple-darwin \
  --repo Typiqally/rclaude --signer-workflow Typiqally/rclaude/.github/workflows/release.yml
gh attestation verify ./SHA256SUMS \
  --repo Typiqally/rclaude --signer-workflow Typiqally/rclaude/.github/workflows/release.yml
shasum -a 256 ./rclaude-VERSION-aarch64-apple-darwin
```

Compare the digest with the same filename in the verified `SHA256SUMS`, then
install the binary with `install -m 755`. The project does not include an updater;
upgrading rclaude does not upgrade the official Claude CLI.
