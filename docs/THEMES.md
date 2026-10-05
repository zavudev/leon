# Themes

A theme is a look: colours, fonts, corner radii and the blueprint lines of
Leon's grid. Leon has two built in (`leon`, the default, and `zavu`) and reads
any number more from plain text files, one per theme. Light or dark (the
*appearance*) is a separate choice that works with every theme: a theme file
defines both.

The product's mark (the glare in the header) and the application icon are
**not** themeable: the theme only sets the colour the mark is drawn in
(`logo`), never its shape.

## Where the files go

One TOML file per theme, in a `themes` folder inside Leon's data directory, the
folder that holds `settings.json`:

| Platform | Folder |
| --- | --- |
| macOS | `~/Library/Application Support/leon/themes/` |
| Linux | `$XDG_DATA_HOME/leon/themes/` (`~/.local/share/leon/themes/`) |
| Windows | `%APPDATA%\leon\themes\` |

With `--data-dir <path>`, it is `<path>/themes/`. The command **Open themes
folder** (palette: `Cmd+Shift+P`, type "themes folder") creates the folder if
it is missing and shows it in the file manager.

## Make one

1. Run **New theme from current…** in the palette and give it a name. Leon
   writes `themes/<name>.toml`: a file that `extends` the theme you are
   wearing and lists every token, commented out, with its current value and a
   line saying what it is for. It is shown in the file manager.
2. Uncomment and change what you want. Save.
3. The window follows the file as you save it (see "Live editing"). Choose the
   theme with **Choose theme…**; yours is listed with the others, marked as a
   user theme.

**Export current theme** writes a complete standalone file (no `extends`) of the
theme you are wearing, as a starting point or as a backup. The example
`examples/themes/ocean.toml` in the repository is Leon with another accent in a
dozen lines.

## The file

```toml
id = "ocean"            # lower case letters, digits and dashes; never `leon` or `zavu`
name = "Ocean"          # what the palette shows
author = "Ada"          # optional
extends = "leon"        # optional: a built-in or another user theme

[dark]                  # any subset of the colour tokens
accent = "#4DA3FF"

[light]
accent = "#0B5CAD"

[dark.terminal]         # the terminal's colours, per appearance
cursor = "#FFFFFF"      # (a bare [terminal] table is read for both)

[fonts]
mono = "JetBrains Mono"

[shape]
radius = 8.0

[lines]
corner_ticks = false
```

Colours are `#RRGGBB`, or `#RRGGBBAA` for the veil (`scrim`) only. Token names
are the stable public names: they are never renamed. The full list is the
reference at the end of this page.

### Inheritance

With `extends`, only the keys that differ are needed: "Leon with a different
accent" is the example above. A parent may be a built-in theme or another file
of the folder, in any order; a cycle is an error. Without `extends` a file has
to set every token (the terminal's foreground, background, cursor, selection
and find colours are the exception: they follow the palette).

### What follows the accent

A built-in theme computes some tokens from its accent. A theme that sets
`accent` (and not the token itself) gets the same: the primary button, the
accent fill and the logo when the parent had them equal to its accent; the
terminal cursor and the current find match; the terminal's selection and find
colours as tints of the new accent over the page; and the text drawn on the
accent fills, chosen as the most legible of the parent's, the page and the
text. Set one of these tokens yourself and your value wins.

### Fonts

`sans` and `mono` take a family name. The families bundled with Leon (Inter,
JetBrains Mono, Space Grotesk, Geist Mono) and the families installed on the
system are available. A family that is neither is a **warning**, and the
parent's font stays.

### Lines

The blueprint of the grid is part of the theme: `crosshairs` (`off`, `header`
or `all`), `corner_ticks`, `guides`, `empty_motif`, `tick_length`, `weight`,
and the colours `border`, `grid_mark` and `guide`. `leon` draws all of it,
quietly; `zavu` draws only its one crosshair under the sidebar's header.

## What is checked

Loading never panics and never leaves the window unreadable. A theme file is
checked when it is read (at start, after **Reload themes**, and whenever it is
saved), against the same rules the built-in themes meet.

**Errors make the theme invalid.** It is not applied; the palette lists it as
invalid with its first error; **Show theme problems** lists everything.

* the file is not TOML, has no usable `id`, or takes a built-in's id, or an id
  another file has;
* `extends` names a theme that does not exist, or goes round in a cycle;
* a value is the wrong kind, a colour is not `#RRGGBB`, a number is out of its
  range (see the reference) or a choice is not one of its words;
* a file without `extends` that leaves out tokens;
* text below **4.5:1** on the surfaces it is drawn on (`text`, `text_muted`,
  and the labels on the primary and accent fills);
* the accent, the logo and the four state colours below **3:1** on the page;
* terminal text below **4.5:1** on the terminal's page, an ANSI colour below
  **3:1** (the two blacks 1.2:1 and 2:1), the cursor below 3:1, text on the
  selection, on a find match or the page colour on the current match below 3:1;
* two of the accent and the four state colours closer than **10** in CIE76:
  they could be taken for one another.

**Warnings are applied and reported.**

* the built-in themes' stricter numbers (text colours, terminal text at 7:1,
  ANSI colours at 4.5:1, states 20 apart, the labels on fills at 7:1);
* a blueprint line outside its quiet band: rules and guides between 1.1:1 and
  2:1 against the page, crosshairs and ticks between 1.3:1 and 3:1;
* a veil outside 30 to 90 % opacity;
* a font that is not available;
* a key nothing reads, with a "did you mean" for a near miss.

A report line is `file  error|warning  key: what was found (measured X,
required Y)`.

## Live editing

Leon lists the themes folder once a second and reads it again when two listings
in a row agree that a file changed (so a file an editor is still writing is
never read half way). If the changed file is the theme on screen, the whole
window and every terminal are redrawn with it. A broken save keeps the last good
version on screen and says so in the status line; **Show theme problems** has
the details.

The folder is polled, not watched through the operating system, on purpose: it
holds a few small files, a second is soon enough, and a file-watching library
would add dependencies and a different behaviour on each platform for nothing.

## Commands

All are in the palette (`Cmd+Shift+P`).

| Command | What it does |
| --- | --- |
| Choose theme… | Lists every theme: built-in, user (marked) and invalid (with the first error) |
| New theme from current… | Asks for a name, writes a commented file that extends the current theme |
| Export current theme | Writes a complete standalone file of the current theme |
| Open themes folder | Shows the folder, creating it if needed |
| Reload themes | Reads the folder now |
| Show theme problems | Lists every error and warning of every file |

`--theme-name <id>` wears a theme for one run, a user theme included. A saved
`theme_id` whose file is gone falls back to `leon`, and the status line says so.

## Share one

A theme is one file: send it, or put it in a repository. Whoever receives it
puts it in their `themes` folder. Nothing in a file is executed.

## Debug one

1. **Show theme problems** names the file, the key, what was measured and what
   is required.
2. A key that does nothing is usually a typo: the report has the warning, with
   a suggestion.
3. A theme missing from the palette's list is missing from the folder, or its
   file name does not end in `.toml`.
4. **Export current theme** shows every token with its value as Leon sees it.

## Reference

Generated from the token list in `crates/app/src/theme/tokens.rs`; a test fails
when this copy is out of date. Regenerate it with
`cargo test -p leon print_theme_reference -- --ignored --nocapture`.

<!-- tokens:begin -->
#### Colours: `[dark]` and `[light]`

| Key | Holds | Follows when not set | What it is |
| --- | --- | --- | --- |
| `background` | `#RRGGBB` | its parent's | The page: the sidebar, the main pane and their headers. |
| `surface` | `#RRGGBB` | its parent's | Raised pieces on the page: cards, fields, chips. |
| `surface_2` | `#RRGGBB` | its parent's | Recessed or hovered fill: the open row, a pressed control. |
| `border` | `#RRGGBB` | its parent's | Hairline rules between panes and around fields. |
| `grid_mark` | `#RRGGBB` | its parent's | Crosshairs where rules meet and corner ticks: one step stronger than `border`. |
| `guide` | `#RRGGBB` | its parent's | The faintest blueprint line: dimension lines and ticks of an empty state. |
| `scrim` | `#RRGGBBAA` | its parent's | The veil over the window behind an overlay. Translucent: `#RRGGBBAA`. |
| `elevated_border` | `#RRGGBB` | its parent's | The outline of something lifted over the window: a card, a menu. |
| `text` | `#RRGGBB` | its parent's | Names and message text. |
| `text_muted` | `#RRGGBB` | its parent's | Secondary text, labels, metadata. |
| `text_faint` | `#RRGGBB` | its parent's | Glyphs that only decorate, and quiet captions. |
| `accent` | `#RRGGBB` | its parent's | The accent as text and as a line: focus ring, selection bar, caret, active marker. |
| `accent_fill` | `#RRGGBB` | accent when the parent's did | The accent as a fill, behind `on_accent_fill`. |
| `on_accent_fill` | `#RRGGBB` | the most legible of the parent's, the page and the text | What is drawn on `accent_fill`. |
| `primary_fill` | `#RRGGBB` | accent when the parent's did | The fill of a primary button. |
| `on_primary` | `#RRGGBB` | the most legible of the parent's, the page and the text | Text on a primary button. |
| `success` | `#RRGGBB` | its parent's | Operational, connected. A state colour: never decoration. |
| `warning` | `#RRGGBB` | its parent's | Attention, processing. A state colour. |
| `error` | `#RRGGBB` | its parent's | Failure, interruption. A state colour. |
| `info` | `#RRGGBB` | its parent's | Activity, live signals. A state colour. |
| `elsewhere` | `#RRGGBB` | its parent's | A session running in another terminal, not in Leon. A state colour, apart from `success`. |
| `logo` | `#RRGGBB` | accent when the parent's did | The colour of the Leon mark in the header. The mark itself is not themeable. |
| `agent_claude` | `#RRGGBB` | its parent's | Claude Code's mark. |
| `agent_codex` | `#RRGGBB` | its parent's | Codex's mark. |
| `agent_opencode` | `#RRGGBB` | its parent's | opencode's mark. |

#### The terminal: `[dark.terminal]` and `[light.terminal]`

| Key | Holds | Follows when not set | What it is |
| --- | --- | --- | --- |
| `foreground` | `#RRGGBB` | text | Terminal text with no colour of its own. |
| `background` | `#RRGGBB` | background | The terminal's page. |
| `cursor` | `#RRGGBB` | accent | The terminal's cursor. |
| `selection` | `#RRGGBB` | a tint of the accent over the page | The background of selected cells (opaque). |
| `find_match` | `#RRGGBB` | a lighter tint of the accent over the page | The background of a find match (opaque). |
| `find_match_current` | `#RRGGBB` | accent | The background of the match the find bar is on; its text is drawn in the page's colour. |
| `ansi` | 16 x `#RRGGBB` | its parent's | The sixteen ANSI colours: black, red, green, yellow, blue, magenta, cyan, white, then the eight bright ones. |

#### `[fonts]`

| Key | Holds | Follows when not set | What it is |
| --- | --- | --- | --- |
| `sans` | a family name | its parent's | Interface and message text. A family bundled with Leon (Inter, Space Grotesk) or installed on the system; a missing one falls back to the parent's. |
| `mono` | a family name | its parent's | Everything technical: labels, metadata, code, terminals. A family bundled with Leon (JetBrains Mono, Geist Mono) or installed on the system. |

#### `[shape]`

| Key | Holds | Follows when not set | What it is |
| --- | --- | --- | --- |
| `radius` | 0 to 24 | its parent's | The corner radius of controls, chips, inputs and floating cards, in pixels at 100 %. |
| `radius_cell` | 0 to 24 | its parent's | The corner radius of grid cells and terminal panes. |
| `label_size` | 8 to 16 | its parent's | The size of mono uppercase labels such as `[ STATUS ]`. |
| `crosshair` | 5 to 31 | its parent's | The length of a crosshair's arms where two rules meet. |

#### `[lines]`

| Key | Holds | Follows when not set | What it is |
| --- | --- | --- | --- |
| `crosshairs` | `off`, `header`, `all` | its parent's | Which crosshairs are drawn: `off`, `header` (only under the sidebar's header) or `all` (every intersection of the window's rules and of split panes). |
| `corner_ticks` | `true` or `false` | its parent's | Corner ticks on framed surfaces: cards, menus, the focused pane. |
| `guides` | `true` or `false` | its parent's | The sidebar's footer and the status strip share one rule across the window. |
| `empty_motif` | `true` or `false` | its parent's | A frame of corner ticks and a dimension line around empty states. |
| `tick_length` | 3 to 24 | its parent's | The length of a corner tick's arms, in pixels at 100 %. |
| `weight` | 1 to 2 | its parent's | The thickness of crosshair and tick arms: 1 or 2 pixels at every interface size. |
<!-- tokens:end -->
