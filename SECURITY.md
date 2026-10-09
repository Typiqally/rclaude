# Security policy

## Supported versions

Before the first release, fixes target `main`. Until `1.0`, only the latest
release receives security fixes; older experimental versions are unsupported.

## Reporting a vulnerability

Do not open a public issue for a suspected credential leak or account compromise.
Once private vulnerability reporting is enabled, report privately at:

<https://github.com/Typiqally/rclaude/security/advisories/new>

If private reporting is unavailable, ask the maintainer to enable it without
posting vulnerability details. Include the affected commit/version, reproduction
steps, impact, and any suggested mitigation. Remove access/refresh tokens,
account and session identifiers, Keychain material, private transcripts, and
private repository information.

## Scope

Security-sensitive areas include official OAuth credential access and refresh,
noninteractive Keychain handling, credential-file permissions and replacement,
broker input and output, SSE parsing, upload bounds and cleanup, and remote
session authorization.

Observed upstream interfaces may change or reject legitimate accounts. A
compatibility failure alone is not a vulnerability. Neither a release attestation
nor passing offline tests establishes live protocol support.
