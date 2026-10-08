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
| `fonts/SymbolsNerdFontMono-Regular.ttf` | The icons of the file tree: only symbols, no letters. Drawn as glyphs with the theme's colour tokens (`ui/editor/icons_map.rs`) | Symbols Nerd Font Mono, `NerdFontsSymbolsOnly.tar.xz` of the Nerd Fonts release v3.5.1, <https://github.com/ryanoasis/nerd-fonts>. The glyphs come from Seti-UI, Devicons, Font Awesome, Codicons (CC BY 4.0, Microsoft), Material Design and others; `fonts/SymbolsNerdFont-README.md` is the release's own list of the sets, their versions and licences | The font: MIT, Copyright (c) 2014 Ryan L McIntyre, `fonts/SymbolsNerdFont-LICENSE.txt`; each icon set keeps its own licence, listed in the README beside it |
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
| `crates/leon-den/assets/furniture/*.png` (all 38 pictures of its furniture: desks, tables, chairs, benches, sofas, computers, shelves, the whiteboard, the clock, paintings, plants, the bin, the coffee), `crates/leon-den/assets/tiles/floor_{0..8}.png`, `wall_0.png` | The furniture, the nine floor patterns and the wall set of the Den (the pixel-art view of the live sessions): what its rooms are built from and what its editor offers. Compiled into the binary by `leon-den` (`src/assets.rs`), not by the app. Unchanged files; the floors and the walls are grey and are given the Den's tones in code | `webview-ui/public/assets/` of pixel-agents at commit `3537e14` (2026-08-15), <https://github.com/pixel-agents-hq/pixel-agents> | MIT, Copyright (c) 2026 Pablo De Lucca, `crates/leon-den/assets/LICENSE-pixel-agents-MIT` |
| `crates/leon-den/assets/source/characters/char_{0,2,3,4,5}.png` | The character sheets the lions are made from: build inputs of `cargo run -p leon-den --example make_lions` and of a test, **not drawn by the app**. Unchanged files | `webview-ui/public/assets/characters/` of pixel-agents, which says they are based on MetroCity by JIK-A-4, <https://jik-a-4.itch.io/metrocity-free-topdown-character-pack> | MIT, Copyright (c) 2026 Pablo De Lucca (pixel-agents); MetroCity is CC0 according to its page (to be confirmed before a release) |
| `crates/leon-den/assets/lions/lion_{0..4}_{0..5}.png` | The lions of the Den: each of the five bodies in each of the six head styles (classic mane, lioness with a scarf, cropped mane, crest, glasses, bow). **Generated** by `make_lions` from the character sheets above: never edited by hand. Our changes: the head of every frame replaced by a lion's head drawn in `leon-den/src/atelier.rs` (mane, scarf or bow in three key colours that the app replaces with a shade of the agent's colour; fur in key colours that the app replaces with one of six coats), the skin of the hands recoloured as fur, a tail added, three frames added to each row (asleep, staring, out cold). A test fails when a committed file differs from what the tool writes | derived from the character sheets above | MIT, Copyright (c) 2026 Pablo De Lucca (the bodies, the clothes and the animation); our drawings Apache-2.0 |
| `crates/leon-den/assets/lions/cub.png`, `crates/leon-den/assets/props/*.png` | The cub (a sub-agent), the egg, the nest, the rack, the telescope, the window, the entrance rug with the glare and the framed glare (`lion_painting.png`). **Generated** by `make_lions` from drawings in `leon-den/src/atelier.rs`; the same test checks them | Ours | Apache-2.0 (the glare on the rug and in the frame is the Leon mark: see the trademark note) |

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

## More agent marks

`agents/grok.svg`, `muse.svg` (Meta AI mark), `mimo-code.svg`, `antigravity.svg`,
`pi.svg`, `hermes-agent.svg`, `devin.svg`, `codebuddy.svg`, `kiro.svg`,
`kilocode.svg`, `trae.svg`, `qoder.svg` (also Qoder CLI China) come from
`@lobehub/icons-static-svg` 1.95.1 (<https://github.com/lobehub/lobe-icons>,
MIT), through the Leon website's `public/brand/agents/`; `auggie.svg` is the
Augment logo from the same folder. All draw with `currentColor`. They are
registered by one stem in `brand::agent_marks!` and one `.mark("stem")` on the
agent's row in `leon-core`.

## Agents without a mark

Every other agent of the catalogue (Amp, DeepSeek Harness, ZCode and the
rest) has **no bundled logo**: its mark is multicolour or carries its own
background, which a single-colour tint would turn into a blob, or no licence that
clearly allows redistribution was found. Those
agents get a letter-mark tile, drawn by the application from the theme's tokens
and the initials of the agent's name (`icons::agent_icon`): nothing is bundled
and nothing is fetched at run time. Leon never loads an icon from the network.
