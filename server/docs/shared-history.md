# Shared profile history queries

## Desktop Profiles

In **Server Tools → Profiles**, member search matches the visible display name
or login/email. It does not change the selected member or reveal hidden profile
fields. **Filter history** opens combinable filters; changes apply automatically
after a short typing delay. **Clear** resets history filters and sorting without
clearing the member search.

Response codes accept a specific code (`404`), a class (`4xx`), or `error` for
requests without an HTTP response. Header keys search request headers; param
keys search URL query parameters, not values or body fields. Request dates use
`YYYY-MM-DD` in the desktop's local timezone and include both boundary days.
Invalid dates/codes are shown inline without discarding the last valid results.

Sort by newest/oldest, fastest/slowest, response code, method, hostname, or path.
The list identifies its 20-result limit; filters search all 100 retained entries
for the member in the active workspace.

History loads automatically and follows authorized WebSocket invalidations.
Background updates preserve the selected request while it still matches.
Reconnection catches up on missed updates. The status indicator distinguishes
live, syncing, reconnecting, offline, and failed states; no history refresh
button is needed.

## HTTP API

`GET /api/v1/profiles/:user_id/history?workspace_id=<uuid>` keeps its existing
`[]EntryView` response and authorization rules. Workspace access is required;
reading another user's history also requires `history.read_others`. Filters do
not change those rules.

Optional URL query parameters (empty means no filter):

| Parameter | Meaning |
| --- | --- |
| `method` | Case-insensitive exact HTTP method token, at most 64 bytes. |
| `status` | Exact `100`–`599`, class `1xx`–`5xx`, or `error` (no HTTP response). |
| `hostname` | Case-insensitive substring of the parsed request hostname, excluding port, path, and query. |
| `path` | Case-sensitive substring of the parsed, decoded request URL path, excluding query. |
| `header_keys` | Comma-separated request header names; ALL must match exactly, case-insensitively. Response headers and values are not searched. |
| `param_keys` | Comma-separated decoded request URL query parameter names; ALL must match exactly, case-sensitively. Values and form fields are not searched. |
| `body_type` | `none`, `raw`, `form_url_encoded`, `multipart_form_data`, or `raw:<language>`. |
| `from` | Inclusive `created_at` lower bound, RFC3339. |
| `before` | Exclusive `created_at` upper bound, RFC3339; must be later than `from`. |
| `sort` | `newest` (default), `oldest`, `status_asc`, `status_desc`, `duration_asc`, `duration_desc`, `method`, `hostname`, or `path`. |

Raw languages: `text`, `json`, `jsonl`, `xml`, `html`, `javascript`,
`typescript`, `css`, `markdown`, `graphql`, `yaml`, `toml`, `sql`, `shell`,
`rust`, `python`. `raw` matches all raw languages.

Filters combine with AND. Key lists trim surrounding whitespace and ignore
empty items; each list allows at most 256 nonempty items. Every query value is
bounded to 4096 bytes. Invalid filters return the standard validation error
envelope (HTTP 422). URL-encode query values, including timezone offsets.

Alphabetical sorts ascend (method and hostname case-normalized, path
case-sensitive). Missing responses always sort last for status and duration
in either direction; missing durations also sort last for duration. Ties use
newest date, then descending ID. `oldest` instead uses ascending date and ID.

The server decrypts and filters/sorts the retained history (at most 100 entries
per profile/workspace) **before** selecting at most 20 results. The response
limit, retention, encryption at rest, and response shape are unchanged. No
pagination or total count is added. Existing callers without options retain
their latest-20 behavior and efficient read path.

Existing `shared_history` resource-change websocket events remain scoped to
authorized viewers and identify the profile and workspace. Clients should
reload using their current filters and sort on these events: an insertion,
deletion, or retention eviction can change the selected top 20. Events do not
carry filtered entry bodies or expose additional history.
