# The Den

The Den shows the live sessions as lions at work in a room. The room is yours
to change: move the furniture, put more in, lay carpets, repaint the walls,
make it larger or smaller, or pick another room altogether. This page is about
the room; what the lions do in it is in the README ("The Den").

A lion goes to the piece of furniture that fits what its agent does. So the
furniture is more than decoration: it decides where everybody is.

| The agent | The lion goes to | Which is |
| --- | --- | --- |
| edits, writes, thinks, sleeps | its **own desk**: a work seat | a seat that faces a computer within three tiles; given when the lion joins and kept |
| reads, searches | a **shelf** | the tile under a bookshelf |
| plans | a **board** | the tile under a whiteboard |
| runs a command | a **rack** | the tile under a rack |
| fetches from the web | a **lookout** | the tile under a window or a telescope |
| is idle | a **rest seat**, while one is free | a seat that faces no computer: a sofa, a chair |
| waits for you, needs a permission | the **entrance** | the entrance rug |
| a sub-agent, before it hatches | a **nest** | a nest; the lion that sent it out stands beside it |
| a sub-agent at work | a **little seat** | a wooden bench that faces a computer |

**A place holds as many lions as it has spots**: two can read under a double
bookshelf, one stands at each rack, three wait in the line at the entrance,
a sofa seats two. A lion that finds its place full stays at its own desk and
does there what it does (the card and the roster still say what). So the
number of work seats is how many lions the room holds comfortably; the ones
beyond that stand on free floor, apart from each other.

A room that lacks one of these still works: the lion stands on free floor near
where the thing would be and does the same there, and a sub-agent with no
little seat takes a lion's. While you edit, a note over the feed says what is missing,
in the narrator's voice and then plainly:

```text
No shelf. Readers will stand around.
No bookshelf: lions that read or search stand on free floor.
```

## Who is in the den

Every terminal of this window is a lion. So is every Claude Code or Codex
session of this computer that runs somewhere else, in another Leon window or
in a plain terminal, as long as its process names its session and its
transcript is found: open the Den in any window to watch all of them. These
are told by their transcripts alone, and their card says where they run. The
setting `den_elsewhere` leaves them out. What Leon does and does not claim
about them is in the README ("The Den").

## Choose a den

**Choose a den…** in the palette lists the dens by name; **Choose a den** on
the bar over the room shows a picture of each. There are six built in:

| Id | Name | Tiles | Work seats | Little seats | |
| --- | --- | --- | --- | --- | --- |
| `office` | The office | 14 by 11 | 6 | 2 | Three desks for two, a sofa and a nursery. The default. |
| `open-plan` | The open plan | 26 by 15 | 22 | 2 | Rows of desks under a long wall. |
| `library` | The library | 18 by 13 | 8 | 2 | Shelves along the wall and four study desks on green carpets. |
| `server-room` | The server room | 18 by 11 | 6 | 1 | A wall of racks and a cold floor. |
| `lounge` | The lounge | 16 by 11 | 4 | 2 | Sofas round a coffee table, a few armchairs with a screen. |
| `nook` | The nook | 10 by 9 | 2 | 0 | One desk, one sofa, one nest. |

More lions than seats is fine: the rest work standing, with a tablet.

Choosing a den never loses one of yours: a den you changed is a file of its
own (below) and stays in the list.

## Edit the den

**Edit the Den** (the palette, the View menu, the button on the bar over the
room, or `E` while the Den has the keyboard) opens the editor. The lions go on
living in the room while you change it: move a desk and the lion that sat at
it walks to where its seat went.

A strip under the room holds what there is to put in it, by category, each
piece drawn as it will look; a bar over the room holds the rest.

| To | Pointer | Keyboard |
| --- | --- | --- |
| take a piece in hand | click it in the strip | `.` and `,` step through the strip, `Tab` and `Shift+Tab` through the categories |
| put it down | click a tile; green means it fits, red that it does not | the arrows move the pointer's tile, `Enter` or `Space` clicks it |
| put the hand down | click the piece in the strip again | `Esc` |
| select a piece | click it | move to it, `Enter` |
| move it | drag it | the arrows |
| turn it | **Turn** | `R` (in hand or selected) |
| remove it | **Remove** | `Delete` or `Backspace` |
| lay a carpet, or take one off | the **Floors** strip, then click or drag over tiles | the same, with `.`, the arrows and `Enter` |
| change the whole floor, the walls | **Floor**, **Walls** | `F`, `W` (with `Shift`: the previous one) |
| make the room larger or smaller | **Wider**, **Narrower**, **Deeper**, **Shallower** | `Shift` with an arrow |
| undo, redo | **Undo**, **Redo** | `Ctrl+Z`, `Ctrl+Shift+Z` (`⌘Z`, `⇧⌘Z` on macOS) |
| go back to the built-in den it was made from | **Reset to prefab** (one step to undo) | |
| leave the editor | **Done** | `Esc` (after the hand is empty and nothing is selected), or `E` |

The rules, which the editor enforces and a file is held to:

* A table (a desk, a small, large or coffee table) is a surface: a computer or
  a coffee stands on it. A table moves with what stands on it and is removed
  with it; turn a table bare.
* What hangs on the wall (shelves, boards, windows, pictures, the clock) goes
  on the back wall only.
* A rug lies under everything; anything may stand on it.
* A tall piece (a desk, a plant, a rack) takes floor only where it stands: the
  rows above are drawn behind whoever walks there.
* Nothing solid stands on the way in, the middle tile of the open side.
* The room is 8 to 30 tiles wide and 8 to 19 deep, walls included. The right
  wall and the open side move; a wall does not move onto a piece.
* A seat faces what its picture says (a chair, a sofa), or else the table next
  to it (a bench).

A change that is refused changes nothing, and the note over the feed says why.

## Your dens

A built-in den is never changed. The first change you make to one makes a den
of your own from it, named after it ("My office"), and that is the den in use
from then on; every later change is written to it at once.

| Command | |
| --- | --- |
| **Save the den as…** | A new den of yours with the room as it is: how a den, built in or yours, is duplicated. |
| **Rename the den…** | Another name for the den in use, and for its file. |
| **Delete the den…** | Removes the den in use, after asking. The den it was made from takes its place. |
| **Open dens folder** | Shows the folder in the file manager. |

### Where the files go

One JSON file per den, in a `dens` folder inside Leon's data directory, the
folder that holds `settings.json`:

| Platform | Folder |
| --- | --- |
| macOS | `~/Library/Application Support/leon/dens/` |
| Linux | `$XDG_DATA_HOME/leon/dens/` (`~/.local/share/leon/dens/`) |
| Windows | `%APPDATA%\leon\dens\` |

With `--data-dir <path>`, it is `<path>/dens/`. The den in use is the setting
`den`: a built-in den's id, or the name of a file without `.json`.

**Sharing a den** is copying its file: there is no export or import command.
Put a file from somebody else in the folder (its name in lower-case letters,
digits and dashes, and not a built-in den's id) and it is in the list. Leon
reads a den's file when the Den opens, so a file you edit by hand shows the
next time you open it.

## The file

```json
{
  "version": 1,
  "name": "Cave",
  "based_on": "office",
  "cols": 12,
  "rows": 9,
  "floor": "stone",
  "wall": "moss",
  "carpets": [
    { "x": 1, "y": 6, "w": 4, "h": 3, "style": "red" }
  ],
  "items": [
    { "id": "desk", "x": 4, "y": 3 },
    { "id": "pc", "x": 4, "y": 3 },
    { "id": "bench", "x": 4, "y": 5 },
    { "id": "chair", "x": 8, "y": 6, "turn": 1 },
    { "id": "double_bookshelf", "x": 1, "y": 0 }
  ]
}
```

| Key | |
| --- | --- |
| `version` | The format: `1`. |
| `name` | What the den is called. |
| `based_on` | The built-in den it was made from: what "Reset to prefab" goes back to. Optional. |
| `cols`, `rows` | The size of the room in tiles, walls included. Row 0 and row 1 are the back wall, the first and last columns the side walls, the last row is open. The only keys a file must have. |
| `floor`, `wall` | The floor and the walls, by id (below). |
| `carpets` | Patches of another floor, the lowest first: `x`, `y`, `w`, `h` in tiles and `style`, a floor's id. |
| `items` | The pieces: `id` (the catalogue, below), `x` and `y` of the top left tile of its footprint, and `turn`, which of its views (0 when left out). |

**Reading a file never fails.** What Leon does not understand it leaves out
or puts right, and the status line says the first thing it changed: a piece
with an unknown id is dropped, a piece outside the room is moved inside, one
that lands on another is dropped, an unknown floor or wall is the default, a
room out of bounds is cut to the limits. A file that is no den at all (not
JSON, no size) gives The office, and says so; the file is left as it is.

### Floors and walls

Floors (also the styles of carpets): `wood` (wooden planks), `walnut`, `pine`,
`stone` (slabs), `slate`, `sand` (sandstone), `checker` (checkerboard), and the
carpets `red`, `green`, `blue`, `plum`, `ochre`.

Walls: `rock` (the den's own), `navy`, `plum`, `moss`, `clay`, `ash`.

### The catalogue

Footprints are in tiles, width by height. "Wall" pieces go on the back wall,
"surface" pieces on a table (or the floor).

| Id | Name | Category | Goes on | Footprint | Views | Role |
| --- | --- | --- | --- | --- | --- | --- |
| `desk` | Desk | Tables | floor | 3x2 | 2 | a surface |
| `small_table` | Small table | Tables | floor | 2x2 | 2 | a surface |
| `table` | Large table | Tables | floor | 3x4 | 1 | a surface |
| `coffee_table` | Coffee table | Tables | floor | 2x2 | 1 | a surface |
| `bench` | Cushioned bench | Seats | floor | 1x1 | 1 | seat |
| `wooden_bench` | Wooden bench | Seats | floor | 1x1 | 1 | little seat |
| `chair` | Cushioned chair | Seats | floor | 1x1 | 4 | seat |
| `wooden_chair` | Wooden chair | Seats | floor | 1x2 | 4 | seat |
| `sofa` | Sofa | Seats | floor | 2x1 | 4 | seat (for two) |
| `pc` | Computer | Machines | surface | 1x2 | 4 | computer: on while a lion works at the seat that faces it |
| `rack` | Rack | Machines | floor | 1x2 | 1 | rack: its lights run while a command does |
| `double_bookshelf` | Bookshelf | Wall | wall | 2x2 | 1 | shelf |
| `bookshelf` | Small shelf | Wall | wall | 2x1 | 1 | shelf |
| `whiteboard` | Whiteboard | Wall | wall | 2x2 | 1 | board |
| `window` | Window | Wall | wall | 2x2 | 1 | lookout |
| `clock` | Clock | Wall | wall | 1x2 | 1 | |
| `painting` | Painting | Wall | wall | 1x2 | 1 | |
| `painting_2` | Portrait | Wall | wall | 1x2 | 1 | |
| `large_painting` | Large painting | Wall | wall | 2x2 | 1 | |
| `lion_painting` | The glare, framed | Wall | wall | 1x2 | 1 | |
| `hanging_plant` | Hanging plant | Wall | wall | 1x2 | 1 | |
| `plant` | Plant | Decor | floor | 1x2 | 1 | |
| `plant_2` | Fern | Decor | floor | 1x2 | 1 | |
| `cactus` | Cactus | Decor | floor | 1x2 | 1 | |
| `large_plant` | Large plant | Decor | floor | 2x3 | 1 | |
| `pot` | Pot | Decor | floor | 1x1 | 1 | |
| `bin` | Bin | Decor | floor | 1x1 | 1 | |
| `coffee` | Coffee | Decor | surface | 1x1 | 1 | |
| `rug` | Entrance rug | Den | floor | 3x2 | 1 | entrance |
| `nest` | Nest | Den | floor | 1x1 | 1 | nest |
| `telescope` | Telescope | Den | floor | 1x2 | 1 | lookout |

The furniture, the floors and the wall tiles are from
[pixel-agents](https://github.com/pixel-agents-hq/pixel-agents) (MIT); the
rack, the window, the telescope, the nest, the entrance rug and the framed
glare are drawn for Leon. Sources and licences are in
`crates/app/assets/ASSETS.md`.
