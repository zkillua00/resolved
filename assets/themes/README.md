# Bundled themes

Resolved Dark is the default fallback. Resolved Light and the following open-source
adaptations are installed into the theme library once. They can be selected,
edited, exported, and deleted like imported CSS themes. Deleting an active entry
returns to Resolved Dark. Deleted entries are not restored on restart.

| Family | Variants | Attribution and license |
| --- | --- | --- |
| Kanagawa | Wave, Dragon, Lotus (light) | [Tommaso Laurenzi](https://github.com/rebelot/kanagawa.nvim), MIT |
| Catppuccin | Latte (light), Frappé, Macchiato, Mocha | [Catppuccin](https://github.com/catppuccin/palette), MIT |
| Tokyo Night | Night, Storm, Moon, Day (light) | [folke](https://github.com/folke/tokyonight.nvim), Apache-2.0 |

Upstream palettes were retrieved on 2026-09-07. Kanagawa uses `lua/kanagawa/colors.lua`
and `lua/kanagawa/themes.lua`; Catppuccin uses `palette.json` (version 1.8.0);
Tokyo Night uses the four `extras/wezterm/tokyonight_*.toml` exports.

These are Resolved adaptations, not unmodified upstream themes. Palette colors
are mapped to application surfaces, controls, HTTP/status semantics, and syntax
tokens. Additional surface and interaction shades are derived from those colors.
Primary foregrounds are selected for contrast. Every third-party CSS file carries
its source URL, modification notice, and full upstream license, so attribution
travels with imported and exported copies. The library also displays author and
license credits under each supplied theme's name.
