# Theme CSS reference

API Tester themes use a small native stylesheet contract. The files look like
CSS, but they are not browser stylesheets: only the selectors, properties, and
values documented here are accepted.

The contract is intentionally strict before API Tester 1.0. A stylesheet written
for an older pre-1.0 build is not migrated or given compatibility defaults when
the contract changes.

## Getting started

The easiest starting point is **Settings → Appearance → Create from template…**
or **Default template** in a theme editor. The bundled
[`api-tester-dark.css`](../assets/themes/api-tester-dark.css) file is the
canonical complete example and reproduces the standard interface.

A complete stylesheet contains exactly these four rules:

```css
:root {
    /* Theme metadata and color tokens */
}

.app {
    /* Global typography, scale, and spacing */
}

.button {
    /* Native button geometry and typography */
}

.editor {
    /* Code editor geometry and typography */
}
```

All four selectors are required and may appear only once. Every documented
property in `.app`, `.button`, and `.editor` is also required, including
properties whose value is `auto` or `inherit`.

## What is supported

- `:root` accepts the documented `--api-*` properties and user-defined helper
  custom properties.
- `.app`, `.button`, and `.editor` accept only their documented properties.
- Comments and extra semicolons inside declaration blocks are accepted.
- Rule order is not significant.
- Unknown selectors, selector combinations, pseudo-classes, at-rules, and
  arbitrary browser CSS properties are rejected.
- The maximum stylesheet size is 256 KiB.

### Custom properties and `var()`

Root values may refer to any declared custom property with `var()`.
Fallback values are supported.

```css
:root {
    --brand: #a78bfa;
    --api-primary: var(--brand);
    --api-primary-hover: var(--missing-hover, #c4b5fd);
}
```

User helper names must begin with `--`, but an unknown name beginning with
`--api-` is rejected. `var()` is available only for `:root` values; class
property values must be literal.

## Value syntax

### Zoom

`.app` accepts `zoom` as a number from `0.5` through `2` or a percentage from
`50%` through `200%`.

Zoom changes the application rem size and scales class lengths, typography, and
rem-based controls. Dimensions that are hard-coded as fixed pixels elsewhere in
the native interface remain fixed; this is not browser page zoom.

### Lengths

Supported lengths are:

- pixels, such as `12px`;
- rems, such as `0.75rem`;
- unitless `0`.

Viewport, percentage, em, and other browser units are not supported. All class
lengths are resolved after applying `.app` zoom. Non-margin lengths cannot be
negative. Resolved margins must remain between `-512px` and `4096px`; resolved
non-negative lengths must remain between `0px` and `4096px`.

### Box shorthand

`margin` and `padding` use standard one-to-four-value box shorthand:

```css
margin: 8px;                 /* all sides */
margin: 8px 12px;            /* vertical | horizontal */
margin: 8px 12px 16px;       /* top | horizontal | bottom */
margin: 8px 12px 16px 20px;  /* top | right | bottom | left */
```

Margins may be negative. Padding may not be negative. `.button` padding also
accepts the single keyword `auto`; it cannot be mixed with other values.

### Font families

A font family is one quoted name or one unquoted identifier sequence:

```css
font-family: "SF Mono";
font-family: Helvetica Neue;
```

Font fallback lists are not supported. `.button` and `.editor` accept
`inherit`; `.app` does not. `auto` is not a font family. API Tester validates
the family syntax, not whether that font is installed.

## `.app`

`.app` controls global typography, the rem-based interface scale, and spacing
around the application canvas.

| Property | Default | Accepted values | Effect |
| --- | --- | --- | --- |
| `zoom` | `100%` | `0.5`–`2` or `50%`–`200%` | Scales the application rem and class lengths. |
| `font-family` | `".SystemUIFont"` | One font family name | Sets the application font. |
| `font-size` | `16px` | `8px`–`48px`; pixels only | Sets the unscaled rem base used by zoom. |
| `margin` | `0` | One to four lengths; negatives allowed | Adds space outside the application canvas. |
| `padding` | `0` | One to four non-negative lengths | Adds space inside the application canvas. |

The effective rem size is `font-size × zoom`.

## `.button`

`.button` applies to native API Tester buttons. An explicit geometry value
overrides button-specific size variants. `auto` preserves the native value for
that property. The rule affects GPUI Component buttons, not every raw clickable
element or other control type.

| Property | Default | Accepted values | Effect |
| --- | --- | --- | --- |
| `border-radius` | `auto` | `auto` or a non-negative length | Sets every button corner radius. |
| `width` | `auto` | `auto` or a non-negative length | Sets a common fixed width. |
| `min-width` | `auto` | `auto` or a non-negative length | Sets a common minimum width. |
| `height` | `auto` | `auto` or a non-negative length | Sets a common fixed height. |
| `margin` | `0` | One to four lengths; negatives allowed | Adds space outside every button. |
| `padding` | `auto` | `auto` or one to four non-negative lengths | Sets common inner spacing. |
| `gap` | `auto` | `auto` or a non-negative length | Sets icon, label, and caret spacing. |
| `font-family` | `inherit` | `inherit` or one font family name | Uses the app font or a button-specific font. |
| `font-size` | `inherit` | `inherit` or a non-negative length | Preserves native size variants or sets one size. |

Example compact square buttons:

```css
.button {
    border-radius: 0;
    width: auto;
    min-width: auto;
    height: 28px;
    margin: 0;
    padding: 0 10px;
    gap: 6px;
    font-family: inherit;
    font-size: 13px;
}
```

## `.editor`

`.editor` applies to request, response, script, and theme code editors. It does
not change unrelated monospace UI such as response-header values or the script
console.

| Property | Default | Accepted values | Effect |
| --- | --- | --- | --- |
| `font-family` | `inherit` | `inherit` or one font family name | Uses the app font or an editor-specific font. |
| `font-size` | `0.875rem` | `inherit` or a non-negative length | Sets editor text size. |
| `border-radius` | `auto` | `auto` or a non-negative length | Preserves each editor frame or sets one radius. |
| `margin` | `0` | One to four lengths; negatives allowed | Adds space outside editor surfaces. |
| `padding` | `0` | One to four non-negative lengths | Adds space inside editor surfaces. |

Editor line height follows the selected font size while retaining the standard
14px text to 20px line-height ratio. Editor context menus continue to use the
application font.

Example monospace editors:

```css
.editor {
    font-family: "Menlo";
    font-size: 13px;
    border-radius: 6px;
    margin: 0;
    padding: 8px;
}
```

## `:root` metadata and color tokens

`--api-theme-name` must be a non-empty quoted string.
`--api-appearance` must be `dark` or `light`. Color properties accept CSS color
values understood by API Tester's color parser, including hex, named, RGB, HSL,
and alpha colors.

“Required” means the token must be declared. “Template default” is the value in
the bundled stylesheet; optional tokens may instead be omitted to use their
semantic runtime fallback.

### Metadata

| Token | Required | Template default | Effect |
| --- | --- | --- | --- |
| `--api-theme-name` | Yes | `"API Tester Material Dark"` | Default library name when importing or saving. |
| `--api-appearance` | Yes | `dark` | Selects the dark or light component baseline. |

### Surfaces and text

| Token | Required | Template default | Effect |
| --- | --- | --- | --- |
| `--api-surface` | Yes | `#14121a` | Window, workspace, tab, table, and accordion background. |
| `--api-surface-lowest` | Yes | `#0f0d14` | Deepest recessed and editor surfaces. |
| `--api-surface-low` | Yes | `#1c1a22` | Navigation, sidebars, group boxes, and low panels. |
| `--api-surface-container` | Yes | `#201e26` | Nested containers, popovers, and active editor line. |
| `--api-surface-high` | Yes | `#2b2931` | Hover surfaces, muted controls, tracks, and thumbs. |
| `--api-surface-highest` | Yes | `#36343c` | Strong hover and elevated secondary surfaces. |
| `--api-foreground` | Yes | `#e6e0eb` | Primary text and default editor foreground. |
| `--api-muted-foreground` | Yes | `#cac4d0` | Secondary text, placeholders, and inactive labels. |
| `--api-outline` | Yes | `#49454f` | Borders, dividers, inputs, table rows, and window outline. |

### Primary interaction

| Token | Required | Template default | Effect |
| --- | --- | --- | --- |
| `--api-primary` | Yes | `#e9ddff` | Primary buttons, active labels, links, and keyword fallback. |
| `--api-primary-hover` | Yes | `#d0bcff` | Hovered primary controls, caret, and property syntax. |
| `--api-primary-active` | Yes | `#b9a6ed` | Pressed primary controls and active links. |
| `--api-primary-foreground` | Yes | `#37265e` | Text and icons on primary surfaces. |
| `--api-selection` | Yes | `#4a4359` | Text selection, selected rows, sidebar entries, and drop targets. |
| `--api-secondary-active` | No | `#5a526a` | Pressed secondary controls; falls back to selection. |
| `--api-secondary-foreground` | No | `#e9def9` | Secondary-control content; falls back to foreground. |

### Status colors

| Token | Required | Template default | Effect |
| --- | --- | --- | --- |
| `--api-danger` | Yes | `#8c1d18` | Destructive actions, errors, and danger status. |
| `--api-danger-hover` | No | `#a52a24` | Hovered danger controls; falls back to danger. |
| `--api-danger-active` | No | `#71120e` | Pressed danger controls; falls back to danger. |
| `--api-danger-foreground` | Yes | `#ffdad6` | Content on danger surfaces. |
| `--api-warning` | Yes | `#f4d06f` | Warnings, modified state, and caution status. |
| `--api-warning-hover` | No | `#ffdc82` | Hovered warning controls; falls back to warning. |
| `--api-warning-active` | No | `#d7b34f` | Pressed warning controls; falls back to warning. |
| `--api-warning-foreground` | Yes | `#332b00` | Content on warning surfaces. |
| `--api-success` | Yes | `#2f6f44` | Successful requests and positive status. |
| `--api-success-hover` | No | `#3d8253` | Hovered success controls; falls back to success. |
| `--api-success-active` | No | `#245c36` | Pressed success controls; falls back to success. |
| `--api-success-foreground` | Yes | `#d0f8d8` | Content on success surfaces. |
| `--api-info` | Yes | `#385f8e` | Informational controls, script output, and 3xx status. |
| `--api-info-hover` | No | `#4771a4` | Hovered info controls; falls back to info. |
| `--api-info-active` | No | `#2c4d77` | Pressed info controls; falls back to info. |
| `--api-info-foreground` | Yes | `#d7e9ff` | Content on informational surfaces. |

### Semantic palette

| Token | Required | Template default | Effect |
| --- | --- | --- | --- |
| `--api-red` | Yes | `#ffb4ab` | HTTP methods, charts, booleans, and bearish status. |
| `--api-red-light` | No | `#ffdad6` | Softer red accent. |
| `--api-green` | Yes | `#4ade80` | HTTP methods, charts, and bullish status. |
| `--api-green-light` | No | `#b8f5c8` | Softer green accent. |
| `--api-blue` | Yes | `#93c5fd` | HTTP methods, charts, and informational accents. |
| `--api-blue-light` | No | `#d7e9ff` | Softer blue accent. |
| `--api-yellow` | Yes | `var(--api-warning)` | HTTP methods and charts. |
| `--api-yellow-light` | No | `#ffecb3` | Softer yellow accent. |
| `--api-magenta` | Yes | `#f0abfc` | HTTP methods and charts. |
| `--api-magenta-light` | No | `#f7d5ff` | Softer magenta accent. |
| `--api-cyan` | Yes | `#67e8f9` | HTTP methods and charts. |
| `--api-cyan-light` | No | `#c9f8ff` | Softer cyan accent. |

### Editor colors

| Token | Required | Template default | Effect |
| --- | --- | --- | --- |
| `--api-editor-background` | No | `var(--api-surface-lowest)` | Code editor canvas; falls back to lowest surface. |
| `--api-editor-foreground` | No | `var(--api-foreground)` | Editor text; falls back to foreground. |
| `--api-editor-active-line` | No | `var(--api-surface-container)` | Caret-line background. |
| `--api-editor-line-number` | No | `#948f9a` | Inactive line numbers. |
| `--api-editor-active-line-number` | No | `var(--api-primary-hover)` | Caret-line number. |

### Syntax highlighting

| Token | Required | Template default | Effect |
| --- | --- | --- | --- |
| `--api-syntax-property` | No | `var(--api-primary-hover)` | Object keys and property names. |
| `--api-syntax-string` | No | `#ffd9e3` | String literals. |
| `--api-syntax-number` | No | `#efb8c8` | Numeric literals. |
| `--api-syntax-boolean` | No | `var(--api-red)` | Boolean and null-like literals. |
| `--api-syntax-keyword` | No | `var(--api-primary)` | Language keywords. |
| `--api-syntax-comment` | No | `#948f9a` | Source comments. |
| `--api-syntax-punctuation` | No | `var(--api-muted-foreground)` | Braces, operators, commas, and punctuation. |
| `--api-syntax-variable` | No | `var(--api-foreground)` | Variables and general identifiers. |
| `--api-syntax-type` | No | `#ccc2dc` | Type names and annotations. |
| `--api-syntax-function` | No | `var(--api-primary)` | Function names and calls. |

### Optional-token omission fallbacks

The values above are template defaults. If an optional token is removed, API
Tester uses the following semantic runtime fallback:

| Optional token | Runtime fallback |
| --- | --- |
| `--api-secondary-active` | `--api-selection` |
| `--api-secondary-foreground` | `--api-foreground` |
| `--api-danger-hover` | `--api-danger` |
| `--api-danger-active` | `--api-danger` |
| `--api-warning-hover` | `--api-warning` |
| `--api-warning-active` | `--api-warning` |
| `--api-success-hover` | `--api-success` |
| `--api-success-active` | `--api-success` |
| `--api-info-hover` | `--api-info` |
| `--api-info-active` | `--api-info` |
| `--api-red-light` | `--api-red` |
| `--api-green-light` | `--api-green` |
| `--api-blue-light` | `--api-blue` |
| `--api-yellow-light` | `--api-yellow` |
| `--api-magenta-light` | `--api-magenta` |
| `--api-cyan-light` | `--api-cyan` |
| `--api-editor-background` | `--api-surface-lowest` |
| `--api-editor-foreground` | `--api-foreground` |
| `--api-editor-active-line` | `--api-surface-container` |
| `--api-editor-line-number` | `--api-muted-foreground` |
| `--api-editor-active-line-number` | `--api-primary-hover` |
| `--api-syntax-property` | `--api-primary-hover` |
| `--api-syntax-string` | `--api-foreground` |
| `--api-syntax-number` | `--api-primary-hover` |
| `--api-syntax-boolean` | `--api-red` |
| `--api-syntax-keyword` | `--api-primary` |
| `--api-syntax-comment` | `--api-muted-foreground` |
| `--api-syntax-punctuation` | `--api-muted-foreground` |
| `--api-syntax-variable` | `--api-foreground` |
| `--api-syntax-type` | `--api-primary-hover` |
| `--api-syntax-function` | `--api-primary` |

## Validation and application

Saving or applying a theme validates the complete stylesheet first. Validation
rejects:

- missing, duplicate, or unknown selectors;
- missing, duplicate, or unknown supported properties;
- invalid metadata, colors, keywords, units, ranges, or negative dimensions;
- undefined or cyclic root custom-property references;
- excessive custom-property expansion or nesting.

Custom-property resolution is bounded to 64 levels, 4,096 substitutions,
64 KiB per resolved value, and 4 MiB of total resolution work.

An invalid draft remains in its editor and never replaces the active theme.
Saving an inactive theme updates its library snapshot without selecting it.
Applying or saving the active theme refreshes the interface only after the new
snapshot is valid.

## Pre-1.0 compatibility

There is deliberately no compatibility layer for old theme CSS before 1.0.
When the stylesheet contract changes, older files may fail validation and must
be updated to the current required selectors and properties. Use the current
Default template as the migration reference.

The implementation source of truth is
[`src/theme/schema.rs`](../src/theme/schema.rs), with parsing rules in
[`src/theme/css.rs`](../src/theme/css.rs).
