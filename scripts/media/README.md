# Media

How the screenshots and clips in the root README are made. Nothing here is
needed to build or run Leon.

## What the images show

* **The demo workspace**: two small git projects (`atlas`, `ledger`) with a
  few worktrees each, built by `setup-demo.sh`.
* **The store**: `crates/leon-core/examples/seed_demo.rs` fills a throwaway
  `--data-dir` with those projects, their worktrees and seven history
  sessions, so the sidebar has something to show.
* **The agents are simulated.** `mock-agents/` holds one script per agent
  (`claude`, `codex`, `opencode`): each draws a plausible frame and waits for
  a key. They are not agents, and the README says so under the gallery.
* **Everything else is Leon**: the tree, the terminals, the transcripts, the
  palette, the dialogs and the themes come from this worktree's release build.

## The rig

* `nested.sh` starts a Hyprland nested inside the current session, parks its
  own window on an invisible headless output of the host (so the guest keeps
  rendering), and creates a 1600x1000 headless output inside the guest to
  capture. The person's desktop and keyboard are not used.
* `launch.sh` runs Leon in a `bwrap` sandbox whose `/home/ada` is the demo
  home, so paths read naturally and nothing real is read or written. The
  window is put fullscreen on the headless output.
* `rig.sh` wraps `hyprctl` and `grim` for that instance; keys go to the
  application's window with Hyprland's `sendshortcut`, so nothing depends on
  which window has the keyboard.
* `record.sh` grabs frames at 20 fps; `encode.sh` turns them into the MP4 and
  the animated WebP of each clip.
* `stills.sh` and `clips.sh` are the scripts of every image in the README;
  `build.sh` runs the whole thing and copies the result into `docs/media/`.

## Regenerating

```sh
cargo build --release -p leon
scripts/media/build.sh
```

Requirements: Linux with a running Hyprland session (the guest is a nested
Hyprland), `bwrap`, `grim`, `hyprctl`, `ffmpeg` built with `libwebp_anim`,
`python3` and `git`.

## Notes

* Keys are sent to the application's window with Hyprland's `sendshortcut`
  dispatcher, so the rig never needs a window to hold the keyboard (and
  `wtype`'s modifiers leak state in the guest). The chords are the
  `Ctrl+Shift` aliases of Leon's shortcuts, because a terminal with the
  keyboard keeps only those (see `crates/app/src/keys.rs`).
* The mock agents set their own OSC title, so the sidebar reads "Claude
  Code", "Codex" or "opencode"; the demo shell rc titles a plain shell
  "Shell".
* A Hyprland with the Lua config manager is assumed for hiding the guest
  window; a legacy config falls back to the legacy dispatcher.
