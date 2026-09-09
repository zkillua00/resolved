# Resolved

## Brand foundation

**Category:** Native, programmable API workspace.

**Official app slogan:** Explore. Automate. Resolve.

**Landing-page slogan:** Your API work, together.

**Core idea:** Bring API exploration, reusable workflows, and collaboration into
one working context.

**Product promise:** Compose requests, understand responses, automate repeatable
work, and carry that work across environments and teammates.

**Positioning:** Resolved is a native API workspace for Linux, macOS, and Windows.
It brings HTTP requests, WebSocket conversations, scripts, tests, environments,
and reusable collections together. Work locally, connect to a self-hosted
collaboration server, or operate the workspace through opt-in MCP tools.

The name stays **Resolved**. It expresses progress toward a working result.
It does not promise automatic fixes or successful API responses: failures,
cancellation, and test results remain explicit.

## Audience and product pillars

For backend, full-stack, API, and integration engineers working individually or
with a team. The shared need is continuity between trying an API, understanding
its behavior, and making that work repeatable.

- **Explore:** Compose HTTP requests, inspect responses, exchange WebSocket
  messages, and switch environment context without rebuilding a request.
- **Automate:** Use pre-request and post-response JavaScript, assertions, saved
  request chaining, snippets, WebSocket automation and replays, and MCP control.
- **Resolve:** Use the returned results and test outcomes to decide what to do
  next. Save and revisit useful work, and collaborate through shared workspaces,
  collections, environment definitions, history, and live change updates.

“Together” includes both the parts of an individual's workflow and collaboration
with teammates. Local workspaces do not require the collaboration server.
Server environment definitions are shared; variable values are per user.
MCP connects external agents to the app; it is not an embedded AI assistant.

Explicit request state, secret redaction, native editors, persistent drafts, and
permission controls support the product promise. Keep these concrete details in
feature explanations rather than making inspection the entire identity.

## Personality

| Be | Do not become |
| --- | --- |
| Composed | Cold, passive, or sterile |
| Exact | Pedantic or intimidating |
| Direct | Blunt, cute, or overly clever |
| Capable | Bloated or theatrically powerful |
| Crafted | Ornamental, precious, or fussy |

## Voice

- Lead with what happened, then state what the user can do next.
- Prefer concrete nouns, plain verbs, and short declarative sentences.
- Distinguish states precisely: authored, saved, resolved, sent, failed,
  cancelled, and returned.
- Use *resolve* sparingly. It is a real operation, not a verbal gimmick.
- Keep errors calm, specific, and actionable.
- Explain boundaries honestly. Stored locally does not mean encrypted.
- Avoid hype, superlatives, startup cliches, hacker slang, mascots, and forced
  jokes.

### Slogan usage and supporting copy

| Surface | Copy |
| --- | --- |
| App welcome screen, product lockup, repository introduction | **Explore. Automate. Resolve.** |
| Landing-page hero | **Your API work, together.** |
| Short descriptor | A native, programmable API workspace. |
| Supporting sentence | Explore, test, and automate APIs in a programmable workspace. |
| Extended description | Work with HTTP and WebSockets, script repeatable workflows, and collaborate through your own server. |

Use the official slogan with sentence capitalization and a period after each
verb. Use the landing-page slogan with its comma and final period. Keep one
slogan per lockup; do not stack both beneath the wordmark. The landing page may
use Explore, Automate, and Resolve as separate feature sections farther down.

## Visual identity

### Wordmark

Use **Resolved** in title case. The concept wordmark uses **STIX Two Text
Semibold**: an editorial serif with enough technical discipline to sit beside
request metadata without resembling interface chrome. Keep the spacing natural
and the capital R unaffected. Do not split the word, add punctuation, hide a
symbol in a letter, or turn the name into a visual riddle.

The compact mark is **Res**, lifted unchanged from the first three glyphs of the
wordmark. It is an editorial label, not an abbreviation used in prose and not a
second logo. The full wordmark remains primary everywhere the available width
allows it.

The in-product interface remains in the native system face. Monospace is
reserved for technical content: URLs, methods, status codes, durations, headers, and
payloads.

### Palette

The external identity is independent from user-selectable application themes.
Its starting palette is deliberately small:

| Role | Name | Value |
| --- | --- | --- |
| Primary signal | Resolve blue | `#3B4BEA` |
| Dark ground | Ink | `#17171B` |
| Light ground | Paper | `#F4F0E7` |
| Secondary surface | Graphite | `#37363D` |
| Quiet accent | Trace | `#A9B0FF` |

Red, green, cyan, and yellow remain available for HTTP and status semantics
rather than becoming decorative brand colors.

### Graphic language

Layouts use alignment, rules, columns, field labels, and real API workflows.
The recurring visual progression is **exploration becoming repeatable work**:

1. Explore: show an editable request or a WebSocket conversation with context.
2. Automate: connect that work to scripts, tests, or reusable sequences.
3. Resolve: show the actual outcome and the saved or shared work it informs.

Use connected steps in explanatory layouts, without adding symbols to the
wordmark or icon. Product imagery should show requests, results, and environment
context together. Show genuine error states as readily as successful responses.

Motion, when used, follows a meaningful action or state change and then holds.
There is no bounce, glow, confetti, or perpetual activity.

## Icon brief

The app icon uses the compact **Res** label in Paper on a Resolve blue rounded
tile. The label is large, left-biased, and optically centered on its baseline.
Its recognition comes from the name and the color field, not from an invented
metaphor. It must remain legible at 32 and 64 pixels; at 16 pixels the blue tile
and light three-letter rhythm are the recognition cue.

Do not replace the label with a lone R. Do not add a period, outline, gradient,
shadow, inset panel, or supporting symbol. In monochrome contexts, use a solid
field and reverse the label out.

Avoid checkmarks, tickets, puzzles, knots, targets, lenses, pixels, braces,
terminal prompts, node diagrams, links, arrows, loops, lightning, rockets,
miniature app windows, and undistinguished R monograms. The descriptor supplies
category clarity; the symbol does not need to explain APIs.

## Core lockups

Official product lockup:

> Resolved
>
> Explore. Automate. Resolve.

Landing-page lockup:

> Resolved
>
> Your API work, together.

Slogans are supporting text, never part of the app icon. Keep the existing
wordmark, compact Res mark, and palette across both uses.

## Asset inventory

- `resolved-wordmark.svg` and `.png`: standalone wordmark; no slogan.
- `resolved-icon.svg` and `.png`: 1024 × 1024 compact icon master and export.
- `resolved-brand-board.svg` and `.png`: identity reference showing both slogan
  roles, the product category, workflow progression, and palette.
- `../../assets/brand/resolved-icon.png`: full-resolution application asset.
- `../../assets/brand/resolved-runtime.png`: 256 × 256 in-app image.

The SVG files are the editable design sources. Regenerate corresponding PNG
exports after visual edits. The SVG wordmark and icon use STIX Two Text Semibold;
render with that font installed. Preserve the small runtime image for native UI
use. Platform packaging icons are maintained separately.
