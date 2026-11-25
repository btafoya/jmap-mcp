# Dependency decisions

Scope: mail tools only, stdio, own tool design, target postmark2jmap (`IMPLEMENTATION_PLAN.md`). Sources: context7 docs for `rmcp` and `schemars`, plus the TS reference server `btafoya/langmail-mcp`.

## Chosen

| Need | Crate | Notes |
|---|---|---|
| MCP server | `rmcp` 3.x | stdio transport. Tool input and output schemas come from `schemars` types (`Tool::with_input_schema` / `with_output_schema`); annotations via `Tool::annotate`. |
| Schemas and validation | `serde` + `schemars` | Doc comments become schema `description`; `deny_unknown_fields` becomes `additionalProperties: false`; `schema_with` / `transform` for overrides. |
| JMAP over HTTP | `reqwest` (rustls, json) | Called directly: session discovery, then `Email/*`, `Mailbox/get`, `Thread/get`, `EmailSubmission/set`. No JMAP client crate; the surface is small and the send path needs exact control over `onSuccessUpdateEmail` and error types. |
| Async runtime | `tokio` | The send guard must be correct under a multi-threaded runtime (see below). |
| Logging | `tracing` + `tracing-subscriber` | Logs go to stderr; stdout belongs to the MCP stdio transport. |

## Constraints worth remembering

- **Send guard.** The TS server's guard (`src/drafts/send-guard.ts`, `spec/SendProtocol.tla`) relies on the single-threaded event loop: reserve and replay lookup run in one synchronous step before the first await. Tokio is multi-threaded, so the reservation needs a `Mutex` held across the check and the insert, and the spec's interleaving model has to be redone for true concurrency.
- **Server specifics.** Take them from postmark2jmap (`~/projects/postmarkapp2jmapserver`: `src/jmap.rs`, `src/jmap_mail.rs`, `src/outbound.rs`), not from Stalwart. Stalwart is not a target.
- **Open item.** rmcp docs show `_meta` on resource contents; I did not find `_meta` on `Tool`. Only matters if a tool ever links to an MCP Apps UI, which is out of scope.

## Deferred (out of scope for now)

- Email body preprocessing: the `langmail` crate (same version as the npm package).
- HTML sanitizing and the email viewer: `ammonia`, plus a hand-written MCP Apps resource.
- Calendar and contacts: `rrule`, `chrono-tz`, a hand-written iCalendar serializer, and a `reqwest` + `quick-xml` CalDAV client.
- Hosted mode, Streamable HTTP, and error tracking (`sentry`).
