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

Every agent session of this window is a lion; a terminal without an agent is
none. So is every Claude Code or Codex
session of this computer that runs somewhere else, in another Leon window or
in a plain terminal, as long as its process names its session and its
transcript is found: open the Den in any window to watch all of them. These
are told by their transcripts alone, and their card says where they run. The
setting `den_elsewhere` leaves them out. What Leon does and does not claim
about them is in the README ("The Den").

## What you can do to a lion

Select a lion (a click on it or on its row of the roster, the arrows or Tab)
and the bar over the room offers **Message**, **Rename**, **Send home** and
**More**. The right button on a lion, `M`, `Shift+F10` or the menu key open
its menu:

| Item | What it does |
| --- | --- |
| Open | Shows the session's terminal (a sub-agent opens its parent's). `Enter` or a double click does the same. |
| Message… (`I`) | Asks for a text and types it into the session's terminal as a prompt. |
| Queued messages… (`Q`) | Lists the messages that wait for it, to take one or all of them back. Only while some wait. |
| Interrupt (`X`) | Sends the agent its interrupt key (Escape), in the middle of a turn. Only for an agent whose key Leon knows: Claude Code and Codex. |
| Rename… (`R`) | Renames the session, as from its row of the sidebar. |
| Pin / Unpin | Pins the session in the sidebar; only for a session the history holds. |
| Send home (`H`) | Stops the agent and leaves the session in the sidebar, asleep, to be woken where it left off. It always asks first. |
| Close | Ends the session and takes it out of the sidebar. It asks while a program runs. |

`?` lists every key of the Den over the room; any key or a click takes the
list away. The keys are read from the same table the list is written from.
While the Den has the keyboard these single keys are the Den's, `?` and `/`
included: the sheet of all of Leon's shortcuts is **Keyboard shortcuts** in
the palette, and the sidebar's filter is its chord.

## Who needs you

A lion needs you when its agent asks for a permission, when it waits (its
turn is over, or it asks a question), or when its session ended with an
error. The roster lists those first, the permission prompts ahead, then
whoever has waited longest, and its heading counts them; so does a button of
the bar. `N` selects the next of them, around the end, and **Go to the next
lion that needs you** in the palette does the same from anywhere in Leon,
opening the Den if it is closed.

**What it asks is on its card, in full.** A permission prompt shows the whole
command, path or address of the call it is about, not the short caption; a
question shows its words and the answers on offer; a plan to approve shows
the plan. Then the card says where to answer: `Enter` opens the session's
terminal. Leon types no answer for you. A transcript does not say which key
an answer has, nor whether the question is still on the screen, so a typed
answer could answer something else.

**Interrupt is only for the middle of a turn.** At the agent's prompt Escape
means something else (it clears what is typed, and twice it opens the
history), so the key is not sent there, nor to a session that is paused,
that ended, or that runs elsewhere. A permission prompt is the middle of a
turn: there Escape refuses it.

While a lion is selected, **Rename**, **Sleep the pane**, **Close the pane**
and the pin commands of the palette act on that lion's session too, the whole
session and not one pane of it. A sub-agent stands for its parent's session.

**A message is typed at the agent's prompt, and nowhere else.** When the
agent has finished its turn and its terminal is quiet, the text is pasted and
Enter follows. While it works, the message waits and is typed when it next
waits at its prompt, one message a turn, oldest first; the lion's card says
how many wait, and the status line says when one was typed or dropped. A
message is never typed while a tool call has no result: that is what a
permission prompt and a question of the agent's look like, and a prompt typed
there would answer them. Messages wait in memory only: quitting Leon, or the
session ending or being sent home, drops them.

**Never two messages in one prompt.** After a message is typed, the next
waits until the session is known to have taken it: it was seen at work, or
its transcript holds the prompt. If neither shows after about twenty looks,
the Enter may have been lost and the text may still sit in the agent's input,
where another message would join it. Leon then types nothing more into that
session and says so; open it and send or clear what is there. The line moves
again when the session starts a turn.

**A message may have several lines.** In the palette, `Shift+Enter` ends a
line and goes on to the next (`Backspace` in an empty line takes the one
before back); `Enter` sends. The lines are pasted as one prompt, so they do
not submit one by one.

**Message the pride** (`P`) sends one message to several lions: those that
wait at their prompt, all that can be messaged, or lions you pick one at a
time. Before anything is typed it says who gets it now, for whom it waits,
and who is left out and why.

**Who cannot be messaged.** A session whose transcript Leon does not follow
(a remote one, an agent Leon cannot read, a session whose own id was not
learned yet): nothing else tells its prompt from a question. A paused session
that was restored and not resumed yet. A session that runs elsewhere, in
another window or terminal: Leon has no terminal of it, so it is only opened.
The status line says which.

**The card's summary.** Under the lion's state, and under what it asks, its
card lists what is known of the session and nothing that is not, in two
groups. *Conversation*: what you first and last wrote to it, the agent's last
words to you, how many messages, how many of yours wait. *Session*: the agent
and its model, the project or folder and the branch checked out there, its
title in the history, for how long it has run here, how many tools it used
and how many failed, how much of the context window is used, how many
sub-agents are out. No model writes it, and no cost is shown: Leon reads an
account's limits, not what one session spent. A card that has no room, or
that would cover its lion, says less from its end and ends in an ellipsis;
what the lion asks is the last thing to go.

## Hatch, send home, wake

**Hatch a lion** (`A`) asks where and which agent, as a new session does,
starts it and stays in the Den with the new lion selected: it joins as an
egg. The agent's own first questions (trusting a folder, signing in) are in
its terminal: `Enter` opens it. With a lion selected, the worktree it works
in is offered first and said to be shared: it is still asked, never taken,
since a second agent there writes to the same files.

A lion that was **sent home** is listed under the roster, in *At home*. A
click on its row, or `W`, wakes it: its agent starts again where the session
left off, and Leon comes back to the Den. When Leon first has to look for
another process that holds the session, its terminal opens a moment later
and stays in front.

## Find a lion

`/` (or `G`) asks which lion and selects it. Type part of its name, of its
state ("waiting", "permission") or of its project or folder.

## Two pictures of the same room

The room is drawn in 2.5D: an isometric office seen from above, in the
colours of your theme, dark or light. Its planes are the theme's surfaces and
its edges the theme's hairlines; the accent only signals (the entrance rug,
who waits for you, the lion you selected). A lion's mane is the colour of its
agent, in one of nine shades of it, and a lioness, who has none, wears a scarf
in it. Coat, cut and shade of the mane, build, glasses, what it wears (a cap,
a headset, a bow, a tie) and shirt follow from the session, so a session
keeps its lion, and the sessions of one agent are not one lion many times:
most of it shows from behind too.

A layout's floor, walls and carpets keep their meaning there without their
pixel colours: a carpet is a zone of another tone, and a floor is ruled as its
style is (planks, tiles, slabs, a checker, or nothing for a carpet).

The pixel art is the other picture of the same room, with the same lions in
the same places. It is what you see:

* with the setting **The Den in 2.5D** (`den_3d`) off;
* on a computer whose graphics card cannot draw the room. Leon tries once
  and says why: in the status line when it happens, under the setting, and
  at the end of the keys of the Den (`?`).

Turning the setting on or off changes the picture at once.

The room is edited in the picture it is shown in. In 2.5D the pointer takes a
piece by any part of it that shows: where two overlap, the one in front. A
piece in hand follows the floor under the pointer (one that stands on a table
follows the table's top, one that hangs follows the back wall), and is drawn
where it would go, green where it may and red where it may not. A piece that
is dragged keeps the height it was taken by, so it does not jump. The
selected piece has its edges in the accent. The pictures of the pieces and of
the dens in the strip are drawn the same way, and a floor is a swatch of the
tone the room lays it in.

Lions that have to share a tile, when the floor is full, stand side by side
on it. Those that wait for you stand in a line across the entrance, each
wholly in view, three to a line. A name plate that would cover another plate or another lion's head
goes down a few lines and keeps a line to its lion.

### Looking at a full den

`leon --den` opens on the Den. A development build (`cargo run`) also reads
`LEON_DEN_CAST=<count>`: that many made-up lions, at most 40, each moving
through the states every few seconds, beside the real ones. Nothing can be
done to them: they are nobody's session. A release build ignores it.

```sh
LEON_DEN_CAST=14 cargo run -p leon -- --den --data-dir /tmp/leon-try
```

Without a window, `cargo run -p leon-den --features den3d --example
den_view_png -- DIR` writes pictures of the whole view (a crowd, the editor
with a piece in hand, an empty den, the strip's pictures) into `DIR`.

## Choose a den

**Choose a den…** in the palette lists the dens by name; **Choose a den** on
the bar over the room shows a picture of each. There are six built in:

| Id | Name | Tiles | Work seats | Little seats | |
| --- | --- | --- | --- | --- | --- |
| `office` | The office | 20 by 15 | 12 | 2 | Six desks for two in two rows, a sofa and a nursery. The default. |
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
