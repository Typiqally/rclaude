# ADR-0001: signed in-process broker engine behind a stable launcher

Status: accepted for the local macOS installer.

macOS gives self-signed callers a code-hash Keychain partition independently of
their designated requirement. Rebuilding the broker invalidated a remembered
grant; restoring the authorized executable restored noninteractive access. A
trusted Apple development or Developer ID identity is not available locally.

The local installer uses an unchanged, certificate-signed Rust launcher as the
broker executable. It updates a separately signed Rust dynamic library inside
the same process. The launcher pins the signing certificate and engine identifier,
copies a bounded regular library into a private temporary directory, verifies
that copy with the fixed system codesign tool, and only then loads it. No paths,
verification bypasses, or signer overrides are accepted from broker frames or
environment variables at runtime. The signing certificate is embedded at build
time; its private key stays nonexportable in the login Keychain.

The launcher and engine have one versioned C entry point taking no arguments and
returning an integer exit status. Rust values and credentials never cross that
ABI. The engine catches Rust panics before returning; it remains loaded until
process exit to keep code for background threads and thread-local destructors
valid. Existing JSON-lines broker bounds and credential isolation remain intact.

The unsafe operations are limited to loading the verified library, resolving the
fixed entry-point symbol, and invoking that versioned C function. Their safety
depends on the trusted signing identity, private verified copy, matched ABI,
panic boundary, and process-long library lifetime. Tests cover refusal before
loading, unsafe source paths, signature requirement injection, ABI invocation,
and source replacement after verification begins.

Installing the launcher requires one explicit native authorization. Normal engine
updates preserve the launcher bytes. Changes to the launcher itself or its pinned
certificate require deliberate reauthorization; no background prompt or ACL
rewrite attempts to avoid that requirement. This is local development signing,
not Developer ID signing, notarization, or a generic plugin mechanism.
