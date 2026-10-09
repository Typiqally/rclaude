# Claude controller observations

Observed locally on 2026-10-09: Claude Desktop 1.37937.3 (ASAR SHA-256 `d9452ff7662ee92fa73e1a7c34476ae29cd25a34daef3dab0848ad96ef56d885`) and official Claude CLI 2.1.295. This implementation was independently written from behavioral and static interface observations; no application source or assets are redistributed.

The CLI has an interactive `--cloud <session>` attachment path. Controller traffic uses `https://api.anthropic.com`, official CLI OAuth, `anthropic-version: 2023-06-01`, and the observed OAuth/CCR beta headers. Credentials and organization metadata remain inside this process. Redirects are disabled.

Observed interfaces:

| Operation | Interface |
| --- | --- |
| List sessions | `GET /v1/code/sessions?limit=100`, `data` array |
| Read history | `GET /v1/code/sessions/<id>/events`, `limit`, `sort_order`, `cursor`; `data` and `next_cursor` |
| Read session | `GET /v1/code/sessions/<id>`, observed `response_shape` wrapper |
| Create session | `POST /v1/code/sessions`, title, initial events, environment ID and config sources |
| Send user/control event | `POST /v1/code/sessions/<id>/events`, payload-wrapped event with stable UUID |
| Subscribe | `GET /v1/code/sessions/<id>/events/stream?from_sequence_num=N`, SSE client/ephemeral events |
| Upload bytes | `POST /v1/files`, multipart field `file`, Files API beta |
| Stage session file | `POST /v1/code/sessions/<id>/files`, file ID and unique `/mnt/user-data/uploads/…` path |
| Clean temporary upload | `DELETE /v1/files/<id>` after staging, on both success and failure |

Worker SDK events and ephemeral stream events become broker events. Client results cannot masquerade as worker completion. Plain user UUID echoes can identify the current turn. Upstream error bodies remain private and become typed error codes. Device-binding refusals require official enrollment.

Static observation does not establish account availability or protocol stability. Session reads worked live; isolated creation returned HTTP 400 and needs diagnosis against legitimate account capabilities. Real upload/readback is pending. Avoid retry loops, interactive credential prompts, or changing request shapes without new evidence.

The native version 1 contract is implemented and locally verified; see
[broker contract](broker-contract.md). Initial live checks failed noninteractive
credential access, then discovery schema validation. A self-signed monolithic
rebuild invalidated its macOS hash-based Keychain grant even with the same
certificate requirement. Restoring the authorized executable restored access.

The local installer now preserves an explicitly authorized launcher and updates
a separately signed engine in the same process (ADR-0001). Fresh UI-disabled
processes retained credential access across engine updates. Corrupted copies and
engines signed with the wrong identifier failed before loading. Launcher or
certificate changes still require explicit authorization.

Live discovery now returns 108 sessions over two bounded broker pages. The
schema mismatch was an archived record with an obsolete worker state; archived
sessions now normalize as stopped. Read-only history paging passes for
representable events, while individual canonical events exceeding the 64 KiB
frame budget return `too_large` without truncation. Complete all-client history,
live catch-up truncation, continuation, interruption, and uploads remain release
checks. Read-only subscriptions accepted snapshot continuation and replay from
zero; 307 replayed durable events had strictly increasing sequences.
Only aggregate counts and fixed diagnostic labels were printed; no credentials,
private identifiers, titles, or transcripts were logged or committed. Existing
conversations were not changed.

The complete read-only sweep checked 108 sessions: 72 histories passed with
32,163 canonical events across 695 broker pages. Thirty-five returned explicit
size errors and one returned `incompatible_version`; their complete history is
unqualified. No conversation was modified.
