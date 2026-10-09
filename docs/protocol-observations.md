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

The native version 1 contract is implemented and locally verified; see [broker contract](broker-contract.md). The updated noninteractive live discovery check returned `authentication_required` on 2026-10-09, so real-service history/continuation qualification remains outstanding.

Follow-up diagnosis: official Keychain metadata matched the expected service and
account, but a UI-disabled read returned `errSecAuthFailed` (-25293). This is an
OS access denial rather than an absent login. The updated broker reports
`credential_access_denied`, preserves its noninteractive guards, and does not
retry prompts. Real-service qualification is still blocked.

After explicit CLI authorization on 2026-10-09, the installed signed build could
read the official login from a fresh process with Keychain UI disabled. Live
discovery then returned `incompatible_version`. A changed executable lost access
despite retaining the same self-signed certificate and designated requirement;
restoring the authorized executable restored access. macOS independently assigns
self-signed code a hash-based Keychain partition. This rules out a promise of
retained authorization across local rebuilds. Cloud history, continuation,
interruption, and all-client qualification remain outstanding. No real session
was changed and no credentials or transcript contents were logged.
