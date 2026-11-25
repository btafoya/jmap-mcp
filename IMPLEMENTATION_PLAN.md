# jmap-mcp implementation plan

Rust MCP server for a JMAP mailbox, own tool design. Reference: the TS server `btafoya/langmail-mcp` (send protocol, tool ideas) and `~/projects/postmarkapp2jmapserver` (the target: a JMAP server in Rust, `src/outbound.rs`, `src/jmap_mail.rs`). Dependency choices: `docs/dependency-research.md`.

## Decisions
- Backend: postmark2jmap (`~/projects/postmarkapp2jmapserver`) is the only target. Stalwart is not a target and is not tested. Code to RFC 8620/8621 and take server specifics from postmark2jmap's `src/jmap.rs`, `src/jmap_mail.rs`, `src/outbound.rs`.
- Auth: Bearer API token. `JMAP_URL`, `JMAP_TOKEN`. postmark2jmap refuses Basic for 2FA accounts and superadmins and takes the account password, so Basic is not used. No auth trait until hosted mode exists. Refuse an `http://` URL unless the host is loopback, so the token never crosses the network in clear.
- Transport: stdio only. Logs go to stderr. Streamable HTTP later.
- Tool surface: own design, not tied to langmail-mcp's snapshot. Names carry the `jmap_` prefix.
- Tools: `jmap_list_mailboxes`, `jmap_search_emails`, `jmap_get_email`, `jmap_get_thread`, `jmap_move_emails`, `jmap_mark_emails`, `jmap_trash_emails`, `jmap_create_draft`, `jmap_update_draft`, `jmap_delete_draft`, `jmap_send_draft`, `jmap_reply_to_message`.
- `jmap_reply_to_message` takes `send` (default false). By default it stages the reply as a draft and the agent must call `jmap_send_draft`. With `send: true` it sends through the same guarded path.
- Stack: `rmcp`, `serde` + `schemars`, `reqwest` (rustls), `tokio`, `tracing`.
- Out of scope: calendar, contacts, attachments, mail rules, unsubscribe, HTML sanitizing/viewer, hosted mode.

## Tool design
- **Annotations.** List, search, get: read-only. Move, mark: idempotent, not destructive. Trash: recoverable, not destructive. `jmap_delete_draft`: destructive. Send and reply: destructive, not idempotent, open-world.
- **Output.** Every tool declares an `output_schema` and returns `structuredContent` plus a short text rendering.
- **Pagination.** `jmap_search_emails`, `jmap_get_thread`, `jmap_list_mailboxes` take `limit` (default 25) and `offset`, and return `total`, `count`, `offset`, `has_more`, `next_offset`. `Email/query` supplies `position` and `total`.
- **Server limits.** Read `maxObjectsInGet`, `maxObjectsInSet` and `maxCallsInRequest` from the JMAP session. Bulk tools (move, mark, trash) chunk by `maxObjectsInSet`.
- **Body size.** Bodies are untrusted content. Cap returned body text (about 25k characters) with an explicit truncation note.
- **Identities.** The client resolves the sending identity with `Identity/get`. With several identities, the tool takes an optional `identity` and otherwise uses the one matching the draft's From; no match is a clear error, never a silent pick.
- **Send results.** `send_draft` and a sending `reply_to_message` return the submission `undoStatus` and any queue reason. postmark2jmap queues mail when the From domain has no server token or unverified DKIM/Return-Path, so "submitted" is not "delivered".
- **Errors.** Tool failures return `isError` results with an actionable message mapped from the JMAP error type (for example `forbiddenFrom`: the From address is not an identity of this account). No internal details leak. An unknown send outcome says to check the Sent folder or the submission status before retrying.
- **Startup.** Fetch the JMAP session at startup and fail fast with a clear message on bad URL, bad token or missing mail and submission capabilities.

## Delivery: one pass
No stages. The whole tool set is built in a single pass and verified once at the end. Within that pass, work in this order only because later pieces depend on earlier ones:

1. Local postmark2jmap: start a dev instance, create an account and API token, and record real JMAP request/response pairs. These are the fixtures for the fake server, so it matches the real one.
2. JMAP client: session discovery and limits, request/response envelope, method errors typed by JMAP error type, request timeout, identity resolution.
3. Send guard: `Mutex`-guarded reservation keyed by draft id (send_draft) or replied-to message id (reply_to_message), tool name part of the key. Reserve before the first await. Release on a JMAP method error carrying an error type (for example `forbiddenFrom`); keep on timeout, reset or 5xx and report "outcome unknown". Replay cache returns the recorded result for a repeated key. TTL 10 minutes. postmark2jmap has no server-side duplicate protection, so this is the only one.
4. Tools: all twelve, on top of the client and guard.
5. MCP server: rmcp stdio wiring, input and output schemas from `schemars`, annotations, startup check.
6. Spec: port `spec/SendProtocol.tla` and its two MC configs from langmail-mcp, with the single-threaded event-loop assumption replaced by true interleaving; `spec/check.sh` runs TLC.
7. Evaluation set: seed a mailbox on the local postmark2jmap and write 10 read-only, independent, verifiable questions (XML question/answer pairs) that need several tool calls each.

## Verification (once, at the end)
- Unit tests for the JMAP client and each tool against an in-process fake JMAP server built from the recorded fixtures (request shape, pagination, chunking, error mapping).
- Send guard tests: concurrent duplicate sends submit once; timeout then retry is refused as outcome-unknown; `forbiddenFrom` releases; replay returns the recorded result; a staged reply and `send_draft` of it do not race.
- TLC passes `AtMostOncePerRequest`, `AtMostOncePerEmail`, `TruthfulResults`.
- Whole tool set run over stdio against the local postmark2jmap, including a queued (undeliverable-domain) send; each server quirk found becomes a test.
- The 10 evaluation questions answered by an agent through the server.
- `cargo fmt`, `cargo clippy`, `cargo test` clean.

## Done when
- All twelve tools work over stdio against postmark2jmap.
- Spec and send-guard tests pass.
- The evaluation questions pass.
- No secrets in the diff.
