# jmap-mcp implementation plan

Rust MCP server for a JMAP mailbox, own tool design. Reference: the TS server `btafoya/langmail-mcp` (send protocol, tool ideas) and `~/projects/postmarkapp2jmapserver` (a JMAP server in Rust: `src/outbound.rs`, `src/jmap_mail.rs`). Dependency choices: `docs/dependency-research.md`.

## Decisions
- Backend: JMAP-generic (RFC 8620/8621); works with postmark2jmap and Stalwart. Server quirks behind a small adapter, only when a test forces one.
- Auth: Basic, `JMAP_URL`, `JMAP_USERNAME`, `JMAP_PASSWORD`. Auth sits behind a trait so a hosted bearer mode can be added later.
- Transport: stdio only in wave 1. Streamable HTTP later.
- Tool surface: own design, not tied to langmail-mcp's snapshot.
- Wave 1 tools: search_emails, get_email, get_thread, list_mailboxes, move_emails, mark_emails, trash_emails, create_draft, update_draft, delete_draft, send_draft, reply_to_message.
- Stack: `rmcp`, `serde` + `schemars`, `reqwest` (rustls), `tokio`, `tracing`.
- Out of scope for wave 1: calendar, contacts, attachments, mail rules, unsubscribe, HTML sanitizing/viewer, hosted mode.

## Stage 1: Skeleton
**Goal**: stdio MCP server that lists tools and calls `list_mailboxes` against a JMAP server.
**Success Criteria**: JMAP session discovery + `Mailbox/get` work; an MCP client sees the tool.
**Tests**: session parse unit test; `list_mailboxes` against a fake JMAP server in-process.
**Status**: Not Started

## Stage 2: Read and organize
**Goal**: search_emails, get_email, get_thread, move_emails, mark_emails, trash_emails.
**Success Criteria**: each tool maps to the right `Email/query|get|set` calls; errors carry the JMAP error type.
**Tests**: fake-server request/response assertions per tool; one run against a local postmark2jmap.
**Status**: Not Started

## Stage 3: Send guard model
**Goal**: port `spec/SendProtocol.tla` (and its two MC configs) with the event-loop assumption replaced by a threaded one.
**Success Criteria**: TLC passes `AtMostOncePerRequest`, `AtMostOncePerEmail`, `TruthfulResults` under true interleaving.
**Tests**: `spec/check.sh`.
**Status**: Not Started

## Stage 4: Drafts and send
**Goal**: create/update/delete_draft, send_draft, reply_to_message with a `Mutex`-guarded reservation (reserve before the first await, keep it on ambiguous failure, release on a JMAP method error).
**Success Criteria**: concurrent duplicate sends submit once; a timeout leaves the reservation held and reports "outcome unknown".
**Tests**: concurrent-retry test, timeout-then-retry test, `forbiddenFrom` releases, replay returns the recorded result.
**Status**: Not Started

## Stage 5: Real-server check
**Goal**: run the tools against postmark2jmap and Stalwart.
**Success Criteria**: wave-1 tool set passes on both; each server quirk found is one adapter line plus a test.
**Status**: Not Started
