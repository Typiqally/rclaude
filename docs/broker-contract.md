# Native cloud broker contract, version 1

`rclaude broker` uses newline-delimited JSON. Handshake advertises
`sessionDiscovery`, `history`, `transcript`, and `interrupt`, with
`maximumFrameBytes: 65536`. Every complete frame, including its newline, fits
that budget. Authentication and refresh stay in this process; credentials and
raw upstream errors never appear in frames or logs. Keychain UI is disabled.

`list_sessions` accepts nullable `cursor` and `limit` (1–100, default 100).
It follows all upstream pages without filtering the originating native client.
Returned metadata is whitelisted: `id`, `title`, `repositoryUrl`, `createdAtMs`,
`updatedAtMs`, `archived`, and `status`. Repository URLs omit user information,
query parameters, and fragments. Cursors are broker-local opaque snapshot
references; upstream cursors never leave the process.

After `open_session`, `read_history` accepts that original `sessionId`, nullable
`cursor`, and the same bounded limit. The first page freezes a high-water
sequence, activity state, and complete canonical user/assistant history through
that sequence. Following pages reuse this immutable snapshot, even when a new
snapshot is read or the cloud conversation advances. Events contain stable
`id`, `occurredAtMs`, and whitelisted SDK `payload`. Messages from every native
client are retained; private thinking and provider metadata are excluded.

Responses contain `events`, `nextCursor`, `active`, and `throughSequence`.
Pages may contain fewer than the requested limit to fit the frame budget.
Unrepresentable single events return `too_large`; they are never shortened.
Truncated catch-up, looping upstream cursors, duplicate/out-of-order history,
and failure to reach the captured high-water mark return `history_incomplete`.
Expired snapshot cursors return an explicit error. Snapshots are bounded to
16 MiB/100,000 events each, eight per store and 64 MiB total; discovery is also
bounded to 10,000 sessions. Bounds fail explicitly instead of reporting complete
partial history. Close releases that session's history snapshots.

`open_session` returns the original ID. `send_turn` targets that same ID and
returns the supplied user UUID as `messageId` together with `sessionId`.
`subscribe(afterSequence)` requests continuation after the history snapshot,
filters duplicate replay, and forwards only strictly increasing durable sequence
numbers. Unsequenced ephemeral deltas are omitted; durable assistant messages
supply the transcript. Missing durable sequences and truncated catch-up are
explicit stream errors. Every subscription frame carries `sessionId`, `sequence`,
and `payload`. Bluedex correlates its own UUID or matching command lifecycle
before accepting completion; unrelated replay cannot finish the current turn.
`interrupt` sends a control event to the original session. None of these native
operations create or fork a session. The separate optional `create_session`
operation is for explicitly new Bluedex conversations.

## Qualification

On 2026-10-09, 36 Rust and six installer tests passed, including all-client
fixtures, immutable snapshot paging, catch-up failures, UUID echo and replay
filtering, original-session interruption, credential isolation, and signature
verification before engine loading. Formatting, Clippy, and dependency audit
passed. Bluedex adapter fixtures separately cover current-turn correlation.

The installed local macOS launcher was explicitly authorized once. Subsequent
signed engine updates retained the launcher bytes, and fresh UI-disabled
processes still accessed the official login without another credential grant.
Corrupted engines and correctly signed engines with the wrong identifier were
refused before broker startup. This applies to engine updates; replacing the
launcher or its pinned certificate requires explicit reauthorization.

Live discovery returned 108 sessions over two bounded broker pages. Archived
records with obsolete worker states now normalize as stopped. Read-only history
qualification confirmed stable snapshot sequences, canonical event IDs, and
bounded multi-page responses for representable histories. Some histories contain
individual canonical events larger than the 64 KiB frame budget; they return
`too_large` explicitly rather than shortening or silently dropping content.

Read-only subscriptions accepted continuation after a history snapshot and replay
from zero; 307 replayed durable events had strictly increasing sequences. Live
truncated catch-up remains unqualified.

No existing conversation was modified by these checks. Only aggregate counts,
static schema labels, and fixed error codes were printed; credentials, record
identifiers, titles, and transcripts were not logged or committed. Complete
history from every native client, live catch-up truncation, original-session continuation,
interruption, and attachment staging/readback remain release prerequisites.
The broker stays experimental; passing local fixtures does not qualify these
remaining live operations.
