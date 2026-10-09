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

On 2026-10-09 local HTTP fixtures passed all-client discovery and history,
provider paging, snapshot paging, catch-up failure cases, continuation after the
snapshot, UUID echo, replay filtering, original-session interruption, frame
budgets, credential isolation, and noninteractive Keychain behavior. Bluedex's
adapter fixtures separately verify current-turn correlation and original-session
attachment upload.

The updated broker's live discovery attempt returned `authentication_required`
with Keychain interaction disabled. No prompts were retried and no real
conversations were modified. Real Claude discovery, complete history from every
native client, paging/catch-up, continuation, interruption, and credential
isolation remain release prerequisites. The broker stays experimental.
