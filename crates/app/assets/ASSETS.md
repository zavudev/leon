# Bundled assets

Fonts, the agents' marks and the window icon are compiled into the binary
(`src/brand.rs`). The Leon glare is not bundled as a picture: it is painted by the
animated mark of the `leon-mark` crate. Nothing is read from disk or from the network at run time.
The other icon files are build inputs of the packages (see the end).

| File | What | Source | Licence |
| --- | --- | --- | --- |
| `fonts/SpaceGrotesk-Regular.ttf`, `-Medium.ttf`, `-Bold.ttf` | Interface font | Space Grotesk 2.0.0, static TTFs, <https://github.com/floriankarsten/space-grotesk> | SIL OFL 1.1, `fonts/SpaceGrotesk-OFL.txt` |
| `fonts/GeistMono-Regular.ttf`, `-Medium.ttf`, `-SemiBold.ttf` | Labels, metadata, code | Geist Mono, geist-font 1.5.0, <https://github.com/vercel/geist-font> | SIL OFL 1.1, `fonts/GeistMono-OFL.txt` |
| `fonts/Inter-Regular.ttf`, `-Italic.ttf`, `-Medium.ttf`, `-SemiBold.ttf` | Interface font of the Leon themes | Inter, with its licence file, <https://github.com/rsms/inter> | SIL OFL 1.1, `fonts/Inter-LICENSE.txt` |
| `fonts/JetBrainsMono-Regular.ttf`, `-Medium.ttf` | Labels, metadata, code and terminals of the Leon themes | JetBrains Mono, with its licence file, <https://github.com/JetBrains/JetBrainsMono> | SIL OFL 1.1, `fonts/JetBrainsMono-OFL.txt` |
| `brand/leon-mark.svg`, `brand/leon-mark-16.svg` | The glare, **Leon's product mark in every theme**: round 2 of the brand book, direction 4 (chosen by the owner; round-1 direction A, the lion, is retired). One silhouette with cuts, drawn with `currentColor` in a single layer so the toolkit tints it with the `logo` palette token. `leon-mark-16.svg` is the fitted variant for 16 to 20 px. **These two files are no longer drawn by the app**: `crates/leon-mark` paints the same shapes as vector paths (always animated, tinted with the `logo` token) and a test of that crate checks that its rest pose equals the path of each file, point for point. They stay as the owner's source of the shapes and for `scripts/generate-icons.sh` | The owner's round-2 "glare" drawings (`mark.svg`, `mark-16.svg`; the final mark is `brand/logo/final/mark.svg`), fill changed to `currentColor` | Leon / Zavu trademark: used for this product only |
| `brand/app-icon.svg` | The glare on its ink tile: the one source of every application icon. Its accent is set by `ICON_ACCENT` in `scripts/generate-icons.sh` (acid yellow `#FFEA00`, the owner's decision; the file carries the same value) | identical to `brand/logo/final/app-icon.svg` of this repository | Leon / Zavu trademark: used for this product only |
| `icons/app-icon-{16,32,48,64,128,256,512}.png`, `icons/app-icon-macos-{16,32,64,128,256,512,1024}.png`, `icons/app-icon.ico`, `icons/app-icon.icns` | The application icon for every platform. **Generated** by `scripts/generate-icons.sh` from `brand/app-icon.svg`: never edited by hand. The macOS PNGs inset the artwork in the rounded square of the macOS icon grid; the others are the full-bleed square. `app-icon-256.png` is also embedded as the window icon | `brand/app-icon.svg` | Leon / Zavu trademark: used for this product only |
| `linux/dev.zavu.leon.desktop` | Desktop entry, named after the application id | Ours | Apache-2.0 |
| `agents/claude.svg` | Claude mark, `currentColor`, tinted by the agent's theme token | Simple Icons `claude.svg`, <https://github.com/simple-icons/simple-icons> | CC0 1.0 |
| `agents/codex.svg` | OpenAI mark, stands for Codex, `currentColor` | Simple Icons 13.21.0 `openai.svg` (later releases dropped it), <https://github.com/simple-icons/simple-icons> | CC0 1.0 |
| `agents/opencode.svg` | opencode mark: the frame of `opencode-logo-dark.svg`, without the dim inner square, single path | `packages/console/app/src/asset/brand/opencode-logo-dark.svg` of <https://github.com/sst/opencode> | MIT, Copyright (c) 2025 opencode |
| `agents/cursor.svg` | Cursor mark, `currentColor`, tinted by the theme's neutral text colour | Simple Icons 16.34.0 `cursor.svg` (the path only, `fill="currentColor"`), <https://github.com/simple-icons/simple-icons> | CC0 1.0 |
| `agents/copilot.svg` | GitHub Copilot mark, `currentColor`, tinted by the theme's neutral text colour | Simple Icons 16.34.0 `githubcopilot.svg` (the path only, `fill="currentColor"`), <https://github.com/simple-icons/simple-icons> | CC0 1.0 |
| `agents/gemini.svg` | Google Gemini mark, `currentColor`, tinted by the theme's neutral text colour | Simple Icons 16.34.0 `googlegemini.svg` (the path only, `fill="currentColor"`), <https://github.com/simple-icons/simple-icons> | CC0 1.0 |
| `agents/mistral-vibe.svg` | Mistral AI (stands for Mistral Vibe) mark, `currentColor`, tinted by the theme's neutral text colour | Simple Icons 16.34.0 `mistralai.svg` (the path only, `fill="currentColor"`), <https://github.com/simple-icons/simple-icons> | CC0 1.0 |
| `agents/cline.svg` | Cline mark, `currentColor`, tinted by the theme's neutral text colour | Simple Icons 16.34.0 `cline.svg` (the path only, `fill="currentColor"`), <https://github.com/simple-icons/simple-icons> | CC0 1.0 |
| `agents/kimi.svg` | Kimi mark, `currentColor`, tinted by the theme's neutral text colour | Simple Icons 16.34.0 `kimi.svg` (the path only, `fill="currentColor"`), <https://github.com/simple-icons/simple-icons> | CC0 1.0 |
| `agents/qwen-code.svg` | Qwen (stands for Qwen Code) mark, `currentColor`, tinted by the theme's neutral text colour | Simple Icons 16.34.0 `qwen.svg` (the path only, `fill="currentColor"`), <https://github.com/simple-icons/simple-icons> | CC0 1.0 |

The agent marks are trademarks of their owners (Anthropic, OpenAI, opencode, Anysphere, GitHub, Google, Mistral AI, Cline, Moonshot AI, Alibaba).
They are used only to identify the tools Leon starts. The files draw with
`currentColor`; the colour is the theme token of the agent (`agent_claude`,
`agent_codex`, `agent_opencode` in `src/theme/`): Claude's clay `#D97757`
(Simple Icons `Claude`, claude.ai; `#B8583A` on the light theme, where the
official value is below 3:1), and OpenAI's and opencode's marks, which are
officially monochrome, in ink on light (`#0A0A0B`, `#211E1E` =
`opencode-logo-light.svg`) and white on dark (`#FFFFFF`, `#F1ECEC` =
`opencode-logo-dark.svg`).

Every font of every theme is registered at start (`brand::fonts()`), so
switching theme loads nothing. Space Grotesk has no static SemiBold file; GPUI
picks the nearest weight (Bold) when the UI asks for it. JetBrains Mono is
bundled in Regular and Medium only, so the semibold of small mono labels is
drawn Medium in the Leon themes.

The mark is drawn by `crates/leon-mark` (`ui::widgets::mark` in the app). Its
geometry is `leon-mark/src/geometry.rs`: changing the glare means changing the two
`leon-mark*.svg` files and those points together; the test
`at_rest_the_full_mark_is_the_path_of_leon_mark_svg` fails until they agree. The 16 and
32 px icon pictures use the fitted geometry; change the icon accent in
`scripts/generate-icons.sh`. Geist Mono SemiBold is bundled because the
brand sets small uppercase labels at weight 600 or more.

## Animated brand files (not used by the app)

`brand/logo/final/mark-animated.svg` (transparent, accent colour) and
`brand/logo/final/app-icon-animated.svg` (on the ink tile) are for the website,
the README and the docs. They are **generated** by `cargo run -p leon-mark
--example export-svg` from the same geometry and gestures the app paints (a blink,
a double blink, a glance each way and a glare, in a 26 s loop), with the accent and
ink read from `brand/logo/final/app-icon.svg`: never edited by hand, and a test of
`leon-mark` fails when they drift. They are SMIL (no script) and carry a
`prefers-reduced-motion` media query that swaps the animated path for a still one.
Their first frame, last frame and still path are the owner's path to the byte.

## Agents without a mark

Every other agent of the catalogue (Grok, Muse, DeepSeek Harness, ZCode and the
rest) has **no bundled logo**: no mark of it was found whose licence clearly
allows redistribution (Simple Icons, CC0, has none of them, or has a different
product's mark: its `amp` is the AMP web project, not Sourcegraph's Amp). Those
agents get a letter-mark tile, drawn by the application from the theme's tokens
and the initials of the agent's name (`icons::agent_icon`): nothing is bundled
and nothing is fetched at run time. Leon never loads an icon from the network.
