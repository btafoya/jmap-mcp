# Rust port: dependency decisions (D3-D16)

Sources: context7 docs (rmcp, ammonia, schemars, chrono-tz, rrule-rust, ext-apps) and the repo source. Items marked (repo) were not in context7.

| Dep | Crate | Notes |
|---|---|---|
| @modelcontextprotocol/server | `rmcp` 3.x | Stateless: `NeverSessionManager`, `legacy_session_mode: false`. Request parts via `Extension<http::request::Parts>`. Build `tools/list` with `with_raw_input_schema` / `with_raw_output_schema` from `src/mcp/__snapshots__/tool-surface.test.ts.snap`. OPEN: confirm `_meta` on `Tool` (needed for `_meta.ui.resourceUri` on preview_email). |
| @modelcontextprotocol/ext-apps | none (hand-written) | Resource `ui://...`, mime `text/html;profile=mcp-app`, `_meta.ui.csp` / `.domain` on the `contents[]` item (`ResourceContents::with_meta`). Viewer stays TS, embed with `include_str!`. |
| sanitize-html | `ammonia` | Builder: tags, tag_attributes, url_schemes, strip_comments, link_rel, `attribute_filter` (image proxy). Adds `rel=noopener noreferrer` by default; html5ever parsing differs. Golden-test the 9 email-render test files before trusting it (XSS surface). |
| zod | `serde` + `schemars` | Deserialize and validate inputs only; published schemas come from the snapshot. Error messages will differ. |
| rrule | `rrule` crate | Feed UTC/floating DTSTART and keep the TS zone logic (`caldav/recurrence.ts`). The crate itself resolves in DTSTART's TZID (gap: shift forward, fold: earlier instant), which differs from the TS model. `underLimit` callback becomes an iterator loop. |
| @date-fns/tz | `chrono-tz` + `chrono` | `from_local_datetime` -> Single/Ambiguous/None; `GapInfo` for gaps. Does not read system zone, so docs/upstream/date-fns-tz is moot. |
| ical-generator | hand-written serializer (~300 LOC) | `caldav/ical.ts` post-patches its output (RECURRENCE-ID, attendee scheme case, DTSTAMP params, 75-octet folding). Own the bytes. Oracle: `ical.test.ts`. (repo) |
| timezones-ical-library | embedded VTIMEZONE data | Keep Morocco and Jerusalem corrections (docs/upstream/timezones-ical-library). (repo) |
| tsdav | `reqwest` + `quick-xml` (~400-500 LOC) | Only `caldav/client.ts`: RFC 6764 discovery, user addresses, MKCALENDAR, PUT/DELETE, calendar-query REPORT, PROPPATCH (Apple calendar-color). Contacts are JMAP, not CardDAV. Stalwart quirks: empty-collection REPORT (client.ts:320), 403 vs 412 (client.ts:456). `libdav` not verified. (repo) |
| langmail | `langmail` crate 0.11.2 | Same version as npm. |
| pino | `tracing` | |
| @sentry/node | `sentry` | |
| @logtail/* | unresolved | Read `src/logger/logtail-stream.ts`; probably a batching HTTP POST. (repo) |
| @modelcontextprotocol/client, bun | none | Only in `scripts/`, which stays TS (D2). |

## Other decisions
- D1: axum front door first, proxy unported paths to the TS server.
- D2: `scripts/` stays TS; evals drive the Rust binary over HTTP.
- D18: port `src/tools/` + `src/calendar-feeds/` as one piece (13.6k LOC).
- Port `spec/SendProtocol.tla` first: the send guard assumes a single-threaded event loop, tokio is not.
