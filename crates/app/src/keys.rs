//! Every keyboard shortcut of the application, in one table.
//!
//! [`BINDINGS`] is the registry: what a command is called, the keys it has,
//! the section it is listed under and when it applies. The key handling
//! ([`resolve`]), the command palette and the shortcuts sheet all read this
//! table and nothing else, so they cannot drift apart.
//!
//! Keys are written once for every platform: [`Chord::secondary`] is Cmd on
//! macOS and Ctrl elsewhere, and [`Chord::label`] prints a chord the way the
//! platform does (`⌘K`, `Ctrl+K`).
//!
//! The bindings are fixed. They are data, though, not code: a table read from
//! the user's settings could replace or extend this one without anything else
//! changing.
//!
//! # While a terminal has the keyboard
//!
//! A terminal program must receive every key, so while one has the keyboard
//! ([`Context::terminal`]) only a few chords are Leon's; all the others go to
//! the program, `ctrl-d`, `escape`, `tab` and the arrows included. `ctrl-c`
//! is the program's too, unless there is a selection to copy (below).
//! The rule is the same on every platform and is stated by [`kept_in_terminal`]:
//!
//! * **macOS**: every `Cmd` chord of the registry stays Leon's (no terminal
//!   program uses `Cmd`), and nothing else does. `Cmd+C` and `Cmd+V` copy and
//!   paste. `Ctrl` is entirely the program's.
//! * **Linux and Windows**: `secondary` is `Ctrl`, which programs use
//!   (`ctrl-d` ends input, `ctrl-r` searches history). `Ctrl+C` copies when
//!   there is a selection and is the program's when there is not, so a running
//!   command is still interrupted; `Ctrl+Shift+C` always copies. `Ctrl+V`
//!   pastes, and its `Ctrl+Shift` alias remains available. Other Leon chords
//!   must also hold `Shift`, because a terminal cannot tell `ctrl-shift-x` from
//!   `ctrl-x` and so no program uses it. A chord without `Shift` (`ctrl-p`,
//!   `ctrl-1`, `ctrl-w` ...) goes to the program there.
//! * Bare keys (`escape`, `tab`, `j`, `?`, `enter`...) are never Leon's; the
//!   exceptions are the scrollback keys `shift-pageup` and `shift-pagedown`,
//!   which only exist while a terminal has the keyboard ([`When::Terminal`]).
//!
//! The copy fallback is not in the table because it needs the terminal's
//! selection: [`kept_in_terminal`] keeps the chord, and the shell gives a copy
//! chord that finds no selection back to the program.
//!
//! Leave the terminal for the sidebar with `secondary+B` (`Cmd+B`) or, from
//! anywhere including Linux and Windows terminals, `Ctrl+Shift+B`.
//!
//! # Panes and tabs
//!
//! The terminal chords follow iTerm2 on macOS and use `Ctrl+Shift` elsewhere,
//! where a plain `Ctrl+D` or `Ctrl+W` is the program's. Each chord below is
//! written for one platform with [`mac`] or [`other`] in the table.
//!
//! | Command | macOS | Linux and Windows |
//! | --- | --- | --- |
//! | Split right | `Cmd+D` | `Ctrl+Shift+D` |
//! | Split down | `Cmd+Shift+D` | `Ctrl+Shift+O` |
//! | Focus a pane by direction | `Cmd+Opt+arrows` | `Ctrl+Shift+arrows` |
//! | Next / previous pane | `Cmd+]` / `Cmd+[` | `Ctrl+Shift+]` / `Ctrl+Shift+[` |
//! | Resize the divider | `Ctrl+Cmd+arrows` | `Ctrl+Shift+Alt+arrows` |
//! | Equalise | `Ctrl+Cmd+=` | `Ctrl+Shift+G` |
//! | Maximise / restore | `Cmd+Shift+Enter` | `Ctrl+Shift+Enter` |
//! | Close the pane | `Cmd+W` | `Ctrl+Shift+W` |
//! | New tab (a shell here) | `Cmd+T` | `Ctrl+Shift+T` |
//! | Next / previous tab | `Cmd+Shift+]` / `Cmd+Shift+[` | `Ctrl+Shift+PgDn` / `Ctrl+Shift+PgUp` |
//! | Go to tab 1 to 9 | `Cmd+Opt+1` to `9` | `Ctrl+Shift+1` to `9` |
//!
//! **Tab numbers.** iTerm2 uses `Cmd+1` to `9` for tabs. Leon already uses
//! `secondary+1` to `9` to jump between machines, and a command that moves
//! between machines is used more often than one that moves between the tabs
//! of a worktree, so the machines keep the plain chord and the tabs take
//! `Cmd+Opt+n` on macOS and `Ctrl+Shift+n` elsewhere. A test checks that the
//! two sets never answer the same keystroke.
//!
//! The theme toggle moved from `secondary+Shift+T` to `secondary+Shift+Y`, so
//! that `Ctrl+Shift+T` can open a tab where a terminal's own convention puts
//! it. The tree's context menu opens with `Shift+F10`, the menu key or `m`.

use gpui_kit::Keystroke;

/// Something the application can be told to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    // ----- search
    /// Go to a project, a worktree, a session or a machine.
    GoTo,
    /// The command palette.
    Commands,
    /// The palette, searching the whole session history.
    SearchHistory,
    /// Moves the keyboard to the sidebar's project filter.
    FilterProjects,
    /// Shows only the sessions that are active: a live terminal, or an agent
    /// running elsewhere. Turns the setting `sidebar_active_only` on or off.
    ToggleActiveOnly,
    // ----- panes
    /// Jumps to the machine at this place in the sidebar, from 1.
    Machine(u8),
    /// Moves the keyboard to the sidebar.
    FocusSidebar,
    /// Moves the keyboard to the main pane.
    FocusMain,
    /// The next pane.
    NextPane,
    /// The pane before.
    PreviousPane,
    // ----- movement in the focused pane
    /// One row down.
    Down,
    /// One row up.
    Up,
    /// The first row.
    Top,
    /// The last row.
    Bottom,
    /// A page down.
    PageDown,
    /// A page up.
    PageUp,
    /// Opens the row the keyboard is on.
    Open,
    /// Opens a folded row, or goes to its first child.
    Expand,
    /// Folds an open row, or goes to its parent.
    Collapse,
    // ----- create
    /// Starts an agent session where the keyboard is.
    NewSession,
    /// Adds any command line tool as an agent of your own.
    AddAgent,
    /// Removes an agent of your own.
    RemoveAgent,
    /// Resumes the history session the keyboard is on, after asking: a click
    /// on a row that has no terminal starts nothing before it is confirmed.
    ResumeSession,
    /// Resumes the history session the keyboard is on in another worktree of
    /// its project, when its own folder is gone.
    ResumeIn,
    /// Resumes, in a terminal of Leon, a history session that runs in another
    /// terminal; asks first when that is certain.
    ResumeAnyway,
    /// Ends the process that holds a history session in another terminal
    /// (SIGTERM, never forced), then resumes it in a terminal of Leon.
    TakeOver,
    /// Brings forward the terminal application a session runs in (macOS, when
    /// the process tree names it).
    RevealTerminal,
    /// Opens the login shell of a machine in the folder the keyboard is on.
    OpenShell,
    /// Adds a git worktree to a project.
    NewWorktree,
    /// Adds a machine: with a code (the relay) or over SSH.
    AddMachine,
    /// Shares this computer: the code, the paired computers, the switch.
    ShareMachine,
    /// Opens a folder as a project: the system's folder picker on this
    /// computer, the palette's questions on another machine.
    OpenProject,
    /// Clones a git URL into a folder and adds it as a project.
    CloneProject,
    /// Creates a new git repository and adds it as a project.
    NewProject,
    /// Adds a project to a machine by typing its path.
    AddProject,
    /// Removes a project from the list, after asking.
    RemoveProject,
    /// Removes a git worktree, after asking.
    RemoveWorktree,
    /// Renames the project, machine or live terminal the keyboard is on.
    Rename,
    /// Moves the row the keyboard is on one step up in its list: a project,
    /// a worktree or a session (which pins itself there).
    MoveRowUp,
    /// Moves the row the keyboard is on one step down in its list.
    MoveRowDown,
    /// Pins the session the keyboard is on on top of its list.
    PinSession,
    /// Unpins the session the keyboard is on: it goes back to its place by
    /// recency.
    UnpinSession,
    /// Removes the SSH machine the keyboard is on, after asking.
    RemoveMachine,
    /// Changes the name, host, user, port or identity file of an SSH machine.
    EditMachine,
    /// Explains why an SSH machine is offline, and tests it again.
    WhyOffline,
    /// Copies the path of the project or worktree the keyboard is on.
    CopyPath,
    /// Copies the branch name of the worktree the keyboard is on.
    CopyBranch,
    /// Copies the agent's own id of the history session the keyboard is on.
    CopySessionId,
    /// Shows the folder in the system's file manager (this computer only).
    Reveal,
    /// Shows the stored transcript of the history session the keyboard is on
    /// (or that the focused terminal resumed), without starting anything.
    OpenTranscript,
    /// Removes the history session the keyboard is on from the store, after
    /// asking.
    RemoveFromHistory,
    /// Opens the context menu of the row the keyboard is on.
    ContextMenu,
    /// Looks for the logo of the project the keyboard is on again.
    RefreshIcon,
    /// Chooses an image file of this computer as the project's logo.
    ChooseIcon,
    /// Goes back to the logo that was detected.
    ResetIcon,
    // ----- files
    /// Opens a text file of the machine in a tab, asking for its path.
    OpenFile,
    /// Saves the file that has the keyboard.
    SaveFile,
    /// Closes the file that has the keyboard, asking first while it has
    /// changes that were not saved.
    CloseFile,
    /// Opens a file of the project in view by part of its name.
    QuickOpen,
    /// Looks for text in the file that has the keyboard.
    FindInFile,
    /// Looks for text in the file that has the keyboard and replaces it.
    ReplaceInFile,
    /// Looks for text in the files of the project in view.
    SearchProject,
    /// Shows a Markdown file as text, as text and page, or as the page.
    TogglePreview,
    /// Selects the file that has the keyboard in the file tree, opening the
    /// folders above it.
    RevealInTree,
    // ----- terminal
    /// Splits the focused pane, the new one to its right.
    SplitRight,
    /// Splits the focused pane, the new one below it.
    SplitDown,
    /// Moves the focus to the pane on the left.
    FocusPaneLeft,
    /// Moves the focus to the pane on the right.
    FocusPaneRight,
    /// Moves the focus to the pane above.
    FocusPaneUp,
    /// Moves the focus to the pane below.
    FocusPaneDown,
    /// Moves the focus to the next pane of the tab.
    NextSplit,
    /// Moves the focus to the pane before.
    PreviousSplit,
    /// Moves the divider of the focused pane to the left.
    ResizeLeft,
    /// Moves it to the right.
    ResizeRight,
    /// Moves it up.
    ResizeUp,
    /// Moves it down.
    ResizeDown,
    /// Makes all panes of the tab the same size.
    EqualizeSplits,
    /// Maximises the focused pane, or restores the layout.
    ToggleZoom,
    /// The next terminal tab.
    NextTab,
    /// The terminal tab before.
    PreviousTab,
    /// Goes to the terminal tab at this place, from 1.
    Tab(u8),
    /// Closes the focused pane for good, after asking while a program runs in
    /// it: its terminal ends and its row leaves the sidebar with the history
    /// session that belongs to it.
    CloseSession,
    /// Puts the focused pane to sleep: its program is stopped and its terminal
    /// dropped, and the session stays in the sidebar to be resumed.
    SleepSession,
    /// Moves the keyboard into the terminal on screen, or to the last live
    /// session.
    FocusTerminal,
    /// Copies the terminal's selection.
    Copy,
    /// Pastes into the terminal by what the clipboard holds: an image sends
    /// Ctrl+V to the program, text is pasted, copied files paste their paths.
    Paste,
    /// Pastes the clipboard's text (or file paths) whatever else it holds.
    PasteText,
    /// Sends Ctrl+V so that the program reads the clipboard's image.
    PasteImage,
    /// One page back into the terminal's scrollback.
    ScrollPageUp,
    /// One page forward.
    ScrollPageDown,
    // ----- data
    /// Imports the local history again and syncs every project's worktrees.
    Refresh,
    /// Checks that the machine on screen answers, and what it has installed.
    ProbeMachine,
    // ----- view
    /// Dark to light and back.
    ToggleAppearance,
    /// Chooses system, light or dark.
    SetAppearance,
    /// Chooses the theme (Leon, Zavu...), previewing each as the selection
    /// moves.
    ChooseTheme,
    /// The next theme in the list, wrapping.
    NextTheme,
    /// The previous theme in the list, wrapping.
    PreviousTheme,
    /// Chooses the size of the interface.
    SetInterfaceSize,
    /// The Settings screen.
    Settings,
    /// The usage view: how much of each agent's limits is left.
    ShowUsage,
    /// Reads the agents' usage limits again now.
    RefreshUsage,
    /// The Den: every live session as a lion at work, in the main pane.
    ShowDen,
    /// The home: the actions that start things and the sessions at a glance.
    GoHome,
    /// Opens the editor of the Den's room, or leaves it.
    EditDen,
    /// Chooses the den: a built-in one or one of the user's.
    ChooseDen,
    /// Saves the room as a new den of the user's.
    SaveDenAs,
    /// Renames the user's den in use.
    RenameDen,
    /// Deletes the user's den in use.
    DeleteDen,
    /// Shows the dens folder in the file manager.
    OpenDensFolder,
    /// Explains why a session may be missing: where each agent's history is
    /// looked for and what was found.
    WhyMissing,
    /// Offers the terminals that were open last time again.
    RestoreSessions,
    /// Opens `settings.json` in the system's editor.
    OpenSettingsFile,
    /// Shows the folder that holds `settings.json` in the file manager.
    RevealSettingsFolder,
    /// The interface one step larger.
    Larger,
    /// One step smaller.
    Smaller,
    /// Back to the design's size.
    ActualSize,
    // ----- application
    /// The list of shortcuts.
    Shortcuts,
    /// Closes what is open over the panes, or goes back one step.
    Close,
    /// Quits the application, after asking while a program runs in a terminal.
    Quit,
    /// Closes the window, which is the application's only one: the same as
    /// quitting.
    CloseWindow,
    /// The About panel.
    About,
    // ----- updates
    /// Asks GitHub whether a newer version is out.
    CheckForUpdates,
    /// Restarts into the update that is ready (downloads it first when it is
    /// only on offer).
    RestartToUpdate,
    /// The release notes of the version on offer.
    ShowReleaseNotes,
    /// Stops offering the version that is on offer.
    SkipVersion,
    /// Opens the release's page in the browser, to download it by hand.
    OpenDownloadPage,
    // ----- terminal: find, clear, copy, save
    /// Opens the find bar of the terminal that has the keyboard.
    Find,
    /// The next match of the find bar.
    FindNext,
    /// The previous match of the find bar.
    FindPrevious,
    /// Clears the scrollback and the screen of the terminal.
    ClearBuffer,
    /// Clears the scrollback and keeps the screen.
    ClearScrollback,
    /// Copies the whole scrollback and the screen as plain text.
    CopyAll,
    /// Copies the visible screen as plain text.
    CopyScreen,
    /// Selects the whole buffer of the terminal.
    SelectAll,
    /// Saves the whole buffer as plain text to a file.
    SaveOutput,
    /// Saves the whole buffer with its colours, as ANSI text, to a file.
    SaveOutputAnsi,
    // ----- sidebar
    /// Shows or hides the sidebar.
    ToggleSidebar,
    /// Shows or hides the file tree of the project in view.
    ToggleFiles,
    /// Makes the sidebar wider by one step.
    WidenSidebar,
    /// Makes the sidebar narrower by one step.
    NarrowSidebar,
    /// Gives the sidebar its default width back.
    ResetSidebarWidth,
    // ----- themes
    /// Writes a commented theme file that extends the theme in use.
    NewThemeFromCurrent,
    /// Writes a complete standalone file of the theme in use.
    ExportTheme,
    /// Shows the themes folder in the file manager.
    OpenThemesFolder,
    /// Reads the themes folder again.
    ReloadThemes,
    /// Lists the errors and warnings of the theme files.
    ShowThemeProblems,
}

/// When a binding applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum When {
    /// Always, whatever has the keyboard, text field included.
    Anywhere,
    /// While no text field and no terminal has the keyboard: a key on its own
    /// is a command only where it cannot be a character.
    Panes,
    /// Only while a terminal has the keyboard.
    Terminal,
    /// Only while an editor has the keyboard.
    File,
}

/// The sections of the shortcuts sheet, in its order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    /// Finding things.
    Search,
    /// From one pane or machine to the next.
    Panes,
    /// Moving in the pane that has the keyboard.
    Navigation,
    /// Agent sessions, worktrees, machines and projects.
    Create,
    /// Text files open in tabs.
    Files,
    /// The live terminal sessions.
    Terminal,
    /// Keeping the store fresh.
    Data,
    /// Theme and size.
    View,
    /// The application itself.
    Application,
}

impl Section {
    /// Every section, in the order they are listed.
    pub const ALL: [Section; 9] = [
        Section::Search,
        Section::Panes,
        Section::Navigation,
        Section::Create,
        Section::Files,
        Section::Terminal,
        Section::Data,
        Section::View,
        Section::Application,
    ];

    /// Its heading.
    pub fn title(self) -> &'static str {
        match self {
            Section::Search => "Search",
            Section::Panes => "Panes",
            Section::Navigation => "Navigation",
            Section::Create => "Create",
            Section::Files => "Files",
            Section::Terminal => "Terminal",
            Section::Data => "Data",
            Section::View => "View",
            Section::Application => "Application",
        }
    }
}

/// Which platforms a chord exists on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Only {
    /// Every platform.
    Any,
    /// macOS only.
    Mac,
    /// Linux and Windows only.
    Other,
}

/// One way of pressing a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    /// The key, as GPUI names it: a character (`k`, `+`, `?`) or a name
    /// (`enter`, `up`, `escape`).
    pub key: &'static str,
    /// Cmd on macOS, Ctrl elsewhere.
    pub secondary: bool,
    /// Shift. For a character that needs Shift to be typed at all (`+`, `?`),
    /// it is not asked for: the character is what counts.
    pub shift: bool,
    /// Alt (Option on macOS).
    pub alt: bool,
    /// Ctrl *as well as* Cmd: only means anything on macOS, where Ctrl and
    /// Cmd are different keys (`ctrl+cmd+arrows`).
    pub control: bool,
    /// The platforms it exists on.
    pub only: Only,
}

const fn chord(key: &'static str, secondary: bool, shift: bool) -> Chord {
    Chord {
        key,
        secondary,
        shift,
        alt: false,
        control: false,
        only: Only::Any,
    }
}

/// A key on its own.
const fn key(key: &'static str) -> Chord {
    chord(key, false, false)
}

/// Cmd or Ctrl with a key.
const fn secondary(key: &'static str) -> Chord {
    chord(key, true, false)
}

/// Cmd or Ctrl, Shift and a key.
const fn secondary_shift(key: &'static str) -> Chord {
    chord(key, true, true)
}

/// Shift with a key.
/// Test the connection, in the Connect screen.
pub const CONNECT_TEST: Chord = secondary("enter");
/// Save the machine, in the Connect screen.
pub const CONNECT_SAVE: Chord = secondary("s");
/// Copy the `ssh-copy-id` line, in the Connect screen.
pub const CONNECT_COPY_ID: Chord = secondary_shift("c");
/// Copy the `ssh` line, in the Connect screen.
pub const CONNECT_COPY_SSH: Chord = with_alt(secondary("c"));

const fn shift(key: &'static str) -> Chord {
    chord(key, false, true)
}

/// The chord with Alt added.
const fn with_alt(c: Chord) -> Chord {
    Chord { alt: true, ..c }
}

/// The chord with Ctrl added to Cmd (macOS).
const fn with_control(c: Chord) -> Chord {
    Chord { control: true, ..c }
}

/// The chord, on macOS only.
const fn mac(c: Chord) -> Chord {
    Chord {
        only: Only::Mac,
        ..c
    }
}

/// The chord, on Linux and Windows only.
const fn other(c: Chord) -> Chord {
    Chord {
        only: Only::Other,
        ..c
    }
}

/// Characters that are typed with Shift on most layouts: a chord for one of
/// them matches the character, whatever was held to type it.
fn typed_with_shift(key: &str) -> bool {
    matches!(key, "+" | "?" | ":" | "<" | ">" | "_" | "~" | "!" | "/")
}

/// What Shift makes of a key on a US layout, for the characters a chord may
/// name: the fallback when the platform does not say what was typed.
fn shifted(key: &str) -> Option<&'static str> {
    Some(match key {
        "=" => "+",
        "/" => "?",
        ";" => ":",
        "," => "<",
        "." => ">",
        "-" => "_",
        "`" => "~",
        "1" => "!",
        _ => return None,
    })
}

impl Chord {
    /// Whether `stroke` is this chord on a platform that is, or is not, macOS.
    pub fn matches_on(&self, stroke: &Keystroke, mac: bool) -> bool {
        let held = &stroke.modifiers;
        match self.only {
            Only::Mac if !mac => return false,
            Only::Other if mac => return false,
            _ => {}
        }
        // Ctrl is the secondary key off macOS: a chord asks for one or the
        // other, never both. On macOS Ctrl is a key of its own that a chord
        // may ask for besides Cmd.
        let secondary_ok = if mac {
            held.platform == self.secondary && held.control == self.control
        } else {
            held.control == self.secondary && !held.platform && !self.control
        };
        if !secondary_ok || held.alt != self.alt || held.function {
            return false;
        }
        // The character that was typed is what a character chord is about (so
        // `?` is `?` on every layout); named keys go by their name.
        let named = self.key.chars().count() > 1;
        if named {
            return stroke.key == self.key && held.shift == self.shift;
        }
        let typed = stroke
            .key_char
            .as_deref()
            .filter(|typed| !typed.is_empty() && !self.secondary && !self.alt);
        match typed {
            Some(typed) if typed_with_shift(self.key) => typed == self.key,
            Some(typed) => typed.to_lowercase() == self.key && held.shift == self.shift,
            // With Cmd or Ctrl held nothing is typed: the key's own name.
            // A key that Shift turns into another character (`/` into `?`) is
            // not itself when Shift is held.
            None if typed_with_shift(self.key) => {
                (stroke.key == self.key && (!held.shift || shifted(self.key).is_none()))
                    || (held.shift && shifted(&stroke.key) == Some(self.key))
            }
            None => stroke.key.to_lowercase() == self.key && held.shift == self.shift,
        }
    }

    /// The chord as the platform writes it: `⇧⌘K` on macOS, `Ctrl+Shift+K`
    /// elsewhere.
    pub fn label(&self) -> String {
        self.label_for(crate::platform::is_mac())
    }

    /// [`Self::label`] for a platform that is, or is not, macOS.
    pub fn label_for(&self, mac: bool) -> String {
        let name = key_name(self.key, mac);
        // A shifted character is written as itself: `?`, not `Shift+?`.
        let shift = self.shift && !typed_with_shift(self.key);
        if mac {
            let mut out = String::new();
            if self.control {
                out.push('⌃');
            }
            if self.alt {
                out.push('⌥');
            }
            if shift {
                out.push('⇧');
            }
            if self.secondary {
                out.push('⌘');
            }
            out.push_str(&name);
            out
        } else {
            let mut parts = Vec::new();
            if self.secondary {
                parts.push("Ctrl".to_owned());
            }
            if shift {
                parts.push("Shift".to_owned());
            }
            if self.alt {
                parts.push("Alt".to_owned());
            }
            parts.push(name);
            parts.join("+")
        }
    }
}

/// A key's name as this platform prints it (`Esc`, `↩`).
pub fn key_label(key: &str) -> String {
    key_name(key, crate::platform::is_mac())
}

/// A key's name as it is printed on the key.
fn key_name(key: &str, mac: bool) -> String {
    match key {
        "enter" => if mac { "↩" } else { "Enter" }.to_owned(),
        "escape" => "Esc".to_owned(),
        "backspace" => if mac { "⌫" } else { "Backspace" }.to_owned(),
        "tab" => if mac { "⇥" } else { "Tab" }.to_owned(),
        "up" => "↑".to_owned(),
        "down" => "↓".to_owned(),
        "left" => "←".to_owned(),
        "right" => "→".to_owned(),
        "home" => "Home".to_owned(),
        "end" => "End".to_owned(),
        "insert" => "Insert".to_owned(),
        "pageup" => "PgUp".to_owned(),
        "pagedown" => "PgDn".to_owned(),
        "space" => "Space".to_owned(),
        other => other.to_uppercase(),
    }
}

/// A command and the keys it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    /// What it does.
    pub command: Command,
    /// What it is called, in the palette and the sheet.
    pub label: &'static str,
    /// Where the sheet lists it.
    pub section: Section,
    /// When its keys apply.
    pub when: When,
    /// Its keys: the first is the one shown. Empty for a command that is
    /// reached from the palette only.
    pub chords: &'static [Chord],
    /// Whether the command palette lists it.
    pub palette: bool,
}

const fn bind(
    command: Command,
    label: &'static str,
    section: Section,
    when: When,
    chords: &'static [Chord],
    palette: bool,
) -> Binding {
    Binding {
        command,
        label,
        section,
        when,
        chords,
        palette,
    }
}

use Command as C;
use Section as S;
use When as W;

/// The registry.
pub const BINDINGS: &[Binding] = &[
    // The chords that mean something else outside a terminal come first: while
    // a terminal has the keyboard they win (`Cmd+F` finds, it does not filter
    // the projects; `Cmd+K` clears, it does not go to), and elsewhere they do
    // not apply, so the older meaning stays.
    bind(
        C::Find,
        "Find in the terminal\u{2026}",
        S::Terminal,
        W::Terminal,
        &[mac(secondary("f")), other(secondary_shift("f"))],
        true,
    ),
    bind(
        C::FindNext,
        "Find next",
        S::Terminal,
        W::Terminal,
        &[mac(secondary("g")), other(secondary_shift("e"))],
        true,
    ),
    bind(
        C::FindPrevious,
        "Find previous",
        S::Terminal,
        W::Terminal,
        &[mac(secondary_shift("g")), other(secondary_shift("u"))],
        true,
    ),
    // The same goes for a file: `Cmd+F` finds in the text, it does not filter
    // the projects.
    bind(
        C::FindInFile,
        "Find in the file\u{2026}",
        S::Files,
        W::File,
        &[secondary("f")],
        true,
    ),
    bind(
        C::ReplaceInFile,
        "Replace in the file\u{2026}",
        S::Files,
        W::File,
        &[mac(with_alt(secondary("f"))), other(secondary("h"))],
        true,
    ),
    bind(
        C::ClearBuffer,
        "Clear the terminal buffer",
        S::Terminal,
        W::Terminal,
        &[mac(secondary("k")), other(secondary_shift("x"))],
        true,
    ),
    bind(
        C::ClearScrollback,
        "Clear the scrollback",
        S::Terminal,
        W::Terminal,
        &[mac(with_alt(secondary("k"))), other(secondary_shift("z"))],
        true,
    ),
    bind(
        C::SelectAll,
        "Select all of the terminal",
        S::Terminal,
        W::Terminal,
        &[mac(secondary("a"))],
        true,
    ),
    bind(
        C::SaveOutput,
        "Save the terminal output to a file\u{2026}",
        S::Terminal,
        W::Terminal,
        &[mac(secondary("s"))],
        true,
    ),
    bind(
        C::PasteText,
        "Paste as text",
        S::Terminal,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::PasteImage,
        "Paste image (send Ctrl+V)",
        S::Terminal,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::SaveOutputAnsi,
        "Save the terminal output with colours\u{2026}",
        S::Terminal,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::CopyAll,
        "Copy all terminal output",
        S::Terminal,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::CopyScreen,
        "Copy the visible terminal screen",
        S::Terminal,
        W::Anywhere,
        &[],
        true,
    ),
    // Search
    bind(
        C::GoTo,
        "Go to a project, worktree, session or machine",
        S::Search,
        W::Anywhere,
        &[secondary("p"), secondary("k"), secondary_shift("k")],
        false,
    ),
    bind(
        C::Commands,
        "Command palette",
        S::Search,
        W::Anywhere,
        &[secondary_shift("p")],
        false,
    ),
    bind(
        C::SearchHistory,
        "Search the session history",
        S::Search,
        W::Anywhere,
        &[mac(secondary_shift("f")), other(secondary_shift("i"))],
        true,
    ),
    bind(
        C::FilterProjects,
        "Filter the projects in the sidebar",
        S::Search,
        W::Panes,
        &[key("/"), secondary("f")],
        true,
    ),
    bind(
        C::ToggleActiveOnly,
        "Show only active sessions",
        S::Search,
        W::Anywhere,
        &[],
        true,
    ),
    // Panes
    bind(
        C::Machine(1),
        "Jump to machine 1",
        S::Panes,
        W::Anywhere,
        &[secondary("1")],
        false,
    ),
    bind(
        C::Machine(2),
        "Jump to machine 2",
        S::Panes,
        W::Anywhere,
        &[secondary("2")],
        false,
    ),
    bind(
        C::Machine(3),
        "Jump to machine 3",
        S::Panes,
        W::Anywhere,
        &[secondary("3")],
        false,
    ),
    bind(
        C::Machine(4),
        "Jump to machine 4",
        S::Panes,
        W::Anywhere,
        &[secondary("4")],
        false,
    ),
    bind(
        C::Machine(5),
        "Jump to machine 5",
        S::Panes,
        W::Anywhere,
        &[secondary("5")],
        false,
    ),
    bind(
        C::Machine(6),
        "Jump to machine 6",
        S::Panes,
        W::Anywhere,
        &[secondary("6")],
        false,
    ),
    bind(
        C::Machine(7),
        "Jump to machine 7",
        S::Panes,
        W::Anywhere,
        &[secondary("7")],
        false,
    ),
    bind(
        C::Machine(8),
        "Jump to machine 8",
        S::Panes,
        W::Anywhere,
        &[secondary("8")],
        false,
    ),
    bind(
        C::Machine(9),
        "Jump to machine 9",
        S::Panes,
        W::Anywhere,
        &[secondary("9")],
        false,
    ),
    bind(
        C::FocusSidebar,
        "Focus the sidebar",
        S::Panes,
        W::Anywhere,
        &[
            secondary("l"),
            mac(secondary_shift("b")),
            other(secondary_shift("s")),
        ],
        true,
    ),
    bind(
        C::ToggleSidebar,
        "Show or hide the sidebar",
        S::Panes,
        W::Anywhere,
        &[mac(secondary("b")), other(secondary_shift("b"))],
        true,
    ),
    bind(
        C::ToggleFiles,
        "Show or hide the file tree",
        S::Panes,
        W::Anywhere,
        &[
            mac(secondary_shift("e")),
            other(with_alt(secondary_shift("e"))),
        ],
        true,
    ),
    bind(
        C::WidenSidebar,
        "Make the sidebar wider",
        S::Panes,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::NarrowSidebar,
        "Make the sidebar narrower",
        S::Panes,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ResetSidebarWidth,
        "Reset the sidebar's width",
        S::Panes,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::FocusMain,
        "Focus the main pane",
        S::Panes,
        W::Anywhere,
        &[secondary("j")],
        true,
    ),
    bind(
        C::NextPane,
        "Next pane",
        S::Panes,
        W::Panes,
        &[key("tab")],
        false,
    ),
    bind(
        C::PreviousPane,
        "Previous pane",
        S::Panes,
        W::Panes,
        &[shift("tab")],
        false,
    ),
    // Navigation
    bind(
        C::Down,
        "Down",
        S::Navigation,
        W::Panes,
        &[key("j"), key("down")],
        false,
    ),
    bind(
        C::Up,
        "Up",
        S::Navigation,
        W::Panes,
        &[key("k"), key("up")],
        false,
    ),
    bind(
        C::Top,
        "First row",
        S::Navigation,
        W::Panes,
        &[key("home"), key("g")],
        false,
    ),
    bind(
        C::Bottom,
        "Last row",
        S::Navigation,
        W::Panes,
        &[key("end"), shift("g")],
        false,
    ),
    bind(
        C::PageDown,
        "Page down",
        S::Navigation,
        W::Panes,
        &[key("pagedown")],
        false,
    ),
    bind(
        C::PageUp,
        "Page up",
        S::Navigation,
        W::Panes,
        &[key("pageup")],
        false,
    ),
    bind(
        C::Expand,
        "Expand, or go to the first child",
        S::Navigation,
        W::Panes,
        &[key("l"), key("right")],
        false,
    ),
    bind(
        C::Collapse,
        "Collapse, or go to the parent",
        S::Navigation,
        W::Panes,
        &[key("h"), key("left")],
        false,
    ),
    bind(
        C::ContextMenu,
        "Open the context menu",
        S::Navigation,
        W::Panes,
        &[shift("f10"), key("menu"), key("m")],
        true,
    ),
    bind(
        C::Open,
        "Open the row; a history session resumes in a terminal",
        S::Navigation,
        W::Panes,
        &[key("enter")],
        false,
    ),
    // Create
    bind(
        C::NewSession,
        "New agent session",
        S::Create,
        W::Anywhere,
        &[secondary("n"), secondary_shift("a")],
        true,
    ),
    bind(
        C::AddAgent,
        "Add a custom agent\u{2026}",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::RemoveAgent,
        "Remove a custom agent\u{2026}",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ResumeIn,
        "Resume the session in another worktree…",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ResumeSession,
        "Resume the session…",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ResumeAnyway,
        "Resume here anyway…",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::TakeOver,
        "Take over the session running elsewhere…",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::RevealTerminal,
        "Reveal the terminal it runs in",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::OpenShell,
        "Open a shell here",
        S::Create,
        W::Anywhere,
        &[secondary("t"), secondary_shift("t")],
        true,
    ),
    bind(
        C::NewWorktree,
        "New worktree",
        S::Create,
        W::Anywhere,
        &[secondary_shift("n")],
        true,
    ),
    bind(
        C::AddMachine,
        "Connect a machine…",
        S::Create,
        W::Anywhere,
        &[secondary_shift("m")],
        true,
    ),
    bind(
        C::ShareMachine,
        "Share this machine…",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::OpenProject,
        "Open project…",
        S::Create,
        W::Anywhere,
        &[secondary("o")],
        true,
    ),
    bind(
        C::CloneProject,
        "Clone a repository…",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::NewProject,
        "New project…",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::AddProject,
        "Add a remote project by path…",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::RemoveProject,
        "Remove a project",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::RemoveWorktree,
        "Remove a worktree",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::RemoveMachine,
        "Remove the machine",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::EditMachine,
        "Edit a machine\u{2026}",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::WhyOffline,
        "Why is it offline?",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(C::Rename, "Rename", S::Create, W::Panes, &[key("f2")], true),
    bind(C::MoveRowUp, "Move up", S::Create, W::Panes, &[], false),
    bind(C::MoveRowDown, "Move down", S::Create, W::Panes, &[], false),
    bind(
        C::PinSession,
        "Pin the session",
        S::Create,
        W::Panes,
        &[],
        false,
    ),
    bind(
        C::UnpinSession,
        "Unpin the session",
        S::Create,
        W::Panes,
        &[],
        false,
    ),
    bind(
        C::CopyPath,
        "Copy the path",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::RefreshIcon,
        "Refresh project icon",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ChooseIcon,
        "Choose project icon…",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ResetIcon,
        "Reset project icon",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::CopyBranch,
        "Copy the branch name",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::CopySessionId,
        "Copy the session id",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::Reveal,
        "Reveal in the file manager",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::OpenTranscript,
        "Open the transcript",
        S::Create,
        W::Anywhere,
        &[secondary_shift("l")],
        true,
    ),
    bind(
        C::RemoveFromHistory,
        "Remove from history",
        S::Create,
        W::Anywhere,
        &[],
        true,
    ),
    // Files
    bind(
        C::OpenFile,
        "Open a file\u{2026}",
        S::Files,
        W::Anywhere,
        &[
            mac(secondary_shift("o")),
            other(with_alt(secondary_shift("o"))),
        ],
        true,
    ),
    bind(
        C::SaveFile,
        "Save the file",
        S::Files,
        W::Anywhere,
        &[secondary("s")],
        true,
    ),
    bind(
        C::CloseFile,
        "Close the file",
        S::Files,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::TogglePreview,
        "Preview the Markdown file",
        S::Files,
        W::File,
        &[
            mac(with_alt(secondary("v"))),
            other(with_alt(secondary_shift("v"))),
        ],
        true,
    ),
    bind(
        C::RevealInTree,
        "Reveal the file in the file tree",
        S::Files,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::QuickOpen,
        "Open a file of the project by name\u{2026}",
        S::Files,
        W::Anywhere,
        &[
            mac(with_alt(secondary("p"))),
            other(with_alt(secondary_shift("p"))),
        ],
        true,
    ),
    bind(
        C::SearchProject,
        "Search the text of the project\u{2026}",
        S::Files,
        W::Anywhere,
        &[with_alt(secondary_shift("f"))],
        true,
    ),
    // Terminal
    bind(
        C::FocusTerminal,
        "Focus the terminal",
        S::Terminal,
        W::Anywhere,
        &[secondary("e")],
        true,
    ),
    bind(
        C::SplitRight,
        "Split the pane to the right",
        S::Terminal,
        W::Anywhere,
        &[mac(secondary("d")), other(secondary_shift("d"))],
        true,
    ),
    bind(
        C::SplitDown,
        "Split the pane downwards",
        S::Terminal,
        W::Anywhere,
        &[mac(secondary_shift("d")), other(secondary_shift("o"))],
        true,
    ),
    bind(
        C::FocusPaneLeft,
        "Focus the pane on the left",
        S::Terminal,
        W::Anywhere,
        &[
            mac(with_alt(secondary("left"))),
            other(secondary_shift("left")),
        ],
        true,
    ),
    bind(
        C::FocusPaneRight,
        "Focus the pane on the right",
        S::Terminal,
        W::Anywhere,
        &[
            mac(with_alt(secondary("right"))),
            other(secondary_shift("right")),
        ],
        true,
    ),
    bind(
        C::FocusPaneUp,
        "Focus the pane above",
        S::Terminal,
        W::Anywhere,
        &[mac(with_alt(secondary("up"))), other(secondary_shift("up"))],
        true,
    ),
    bind(
        C::FocusPaneDown,
        "Focus the pane below",
        S::Terminal,
        W::Anywhere,
        &[
            mac(with_alt(secondary("down"))),
            other(secondary_shift("down")),
        ],
        true,
    ),
    bind(
        C::NextSplit,
        "Focus the next pane",
        S::Terminal,
        W::Anywhere,
        &[mac(secondary("]")), other(secondary_shift("]"))],
        true,
    ),
    bind(
        C::PreviousSplit,
        "Focus the previous pane",
        S::Terminal,
        W::Anywhere,
        &[mac(secondary("[")), other(secondary_shift("["))],
        true,
    ),
    bind(
        C::ResizeLeft,
        "Move the pane's divider left",
        S::Terminal,
        W::Anywhere,
        &[
            mac(with_control(secondary("left"))),
            other(with_alt(secondary_shift("left"))),
        ],
        true,
    ),
    bind(
        C::ResizeRight,
        "Move the pane's divider right",
        S::Terminal,
        W::Anywhere,
        &[
            mac(with_control(secondary("right"))),
            other(with_alt(secondary_shift("right"))),
        ],
        true,
    ),
    bind(
        C::ResizeUp,
        "Move the pane's divider up",
        S::Terminal,
        W::Anywhere,
        &[
            mac(with_control(secondary("up"))),
            other(with_alt(secondary_shift("up"))),
        ],
        true,
    ),
    bind(
        C::ResizeDown,
        "Move the pane's divider down",
        S::Terminal,
        W::Anywhere,
        &[
            mac(with_control(secondary("down"))),
            other(with_alt(secondary_shift("down"))),
        ],
        true,
    ),
    bind(
        C::EqualizeSplits,
        "Make the panes the same size",
        S::Terminal,
        W::Anywhere,
        &[
            mac(with_control(secondary("="))),
            other(secondary_shift("g")),
        ],
        true,
    ),
    bind(
        C::ToggleZoom,
        "Maximise or restore the pane",
        S::Terminal,
        W::Anywhere,
        &[secondary_shift("enter")],
        true,
    ),
    bind(
        C::NextTab,
        "Next terminal tab",
        S::Terminal,
        W::Anywhere,
        &[
            mac(secondary_shift("]")),
            other(secondary_shift("pagedown")),
        ],
        true,
    ),
    bind(
        C::PreviousTab,
        "Previous terminal tab",
        S::Terminal,
        W::Anywhere,
        &[mac(secondary_shift("[")), other(secondary_shift("pageup"))],
        true,
    ),
    bind(
        C::Tab(1),
        "Go to terminal tab 1",
        S::Terminal,
        W::Anywhere,
        &[mac(with_alt(secondary("1"))), other(secondary_shift("1"))],
        false,
    ),
    bind(
        C::Tab(2),
        "Go to terminal tab 2",
        S::Terminal,
        W::Anywhere,
        &[mac(with_alt(secondary("2"))), other(secondary_shift("2"))],
        false,
    ),
    bind(
        C::Tab(3),
        "Go to terminal tab 3",
        S::Terminal,
        W::Anywhere,
        &[mac(with_alt(secondary("3"))), other(secondary_shift("3"))],
        false,
    ),
    bind(
        C::Tab(4),
        "Go to terminal tab 4",
        S::Terminal,
        W::Anywhere,
        &[mac(with_alt(secondary("4"))), other(secondary_shift("4"))],
        false,
    ),
    bind(
        C::Tab(5),
        "Go to terminal tab 5",
        S::Terminal,
        W::Anywhere,
        &[mac(with_alt(secondary("5"))), other(secondary_shift("5"))],
        false,
    ),
    bind(
        C::Tab(6),
        "Go to terminal tab 6",
        S::Terminal,
        W::Anywhere,
        &[mac(with_alt(secondary("6"))), other(secondary_shift("6"))],
        false,
    ),
    bind(
        C::Tab(7),
        "Go to terminal tab 7",
        S::Terminal,
        W::Anywhere,
        &[mac(with_alt(secondary("7"))), other(secondary_shift("7"))],
        false,
    ),
    bind(
        C::Tab(8),
        "Go to terminal tab 8",
        S::Terminal,
        W::Anywhere,
        &[mac(with_alt(secondary("8"))), other(secondary_shift("8"))],
        false,
    ),
    bind(
        C::Tab(9),
        "Go to terminal tab 9",
        S::Terminal,
        W::Anywhere,
        &[mac(with_alt(secondary("9"))), other(secondary_shift("9"))],
        false,
    ),
    bind(
        C::CloseSession,
        "Close the pane",
        S::Terminal,
        W::Anywhere,
        &[mac(secondary("w")), other(secondary_shift("w"))],
        true,
    ),
    bind(
        C::SleepSession,
        "Sleep the pane",
        S::Terminal,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::Copy,
        "Copy the selection",
        S::Terminal,
        W::Terminal,
        &[secondary("c"), secondary_shift("c")],
        false,
    ),
    bind(
        C::Paste,
        "Paste",
        S::Terminal,
        W::Terminal,
        &[secondary("v"), secondary_shift("v"), shift("insert")],
        false,
    ),
    bind(
        C::ScrollPageUp,
        "Scroll the terminal back a page",
        S::Terminal,
        W::Terminal,
        &[shift("pageup")],
        false,
    ),
    bind(
        C::ScrollPageDown,
        "Scroll the terminal forward a page",
        S::Terminal,
        W::Terminal,
        &[shift("pagedown")],
        false,
    ),
    // Data
    bind(
        C::Refresh,
        "Import history and sync worktrees",
        S::Data,
        W::Anywhere,
        &[secondary("r")],
        true,
    ),
    bind(
        C::WhyMissing,
        "Why is a session missing?",
        S::Data,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::RestoreSessions,
        "Restore last sessions",
        S::Data,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ProbeMachine,
        "Probe the machine on screen",
        S::Data,
        W::Anywhere,
        &[secondary_shift("r")],
        true,
    ),
    // View
    bind(
        C::ToggleAppearance,
        "Toggle light and dark",
        S::View,
        W::Anywhere,
        &[secondary_shift("y")],
        true,
    ),
    bind(
        C::SetAppearance,
        "Choose appearance\u{2026}",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ChooseTheme,
        "Choose theme\u{2026}",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    // Free on every platform and kept by a terminal: Cmd+Shift on macOS,
    // Ctrl+Shift elsewhere. H and J sit left and right of each other.
    bind(
        C::NextTheme,
        "Next theme",
        S::View,
        W::Anywhere,
        &[secondary_shift("j")],
        true,
    ),
    bind(
        C::PreviousTheme,
        "Previous theme",
        S::View,
        W::Anywhere,
        &[secondary_shift("h")],
        true,
    ),
    bind(
        C::NewThemeFromCurrent,
        "New theme from current\u{2026}",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ExportTheme,
        "Export current theme",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::OpenThemesFolder,
        "Open themes folder",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ReloadThemes,
        "Reload themes",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ShowThemeProblems,
        "Show theme problems",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::SetInterfaceSize,
        "Choose the interface size",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::Settings,
        "Settings",
        S::View,
        W::Anywhere,
        &[secondary(",")],
        true,
    ),
    bind(
        C::OpenSettingsFile,
        "Open settings.json",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::RevealSettingsFolder,
        "Reveal settings folder",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ShowUsage,
        "Show usage",
        S::View,
        W::Anywhere,
        &[
            mac(secondary_shift("u")),
            other(with_alt(secondary_shift("u"))),
        ],
        true,
    ),
    bind(
        C::RefreshUsage,
        "Refresh usage now",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ShowDen,
        "Open the Den",
        S::View,
        W::Anywhere,
        &[with_alt(secondary_shift("l"))],
        true,
    ),
    bind(
        C::GoHome,
        "Go home",
        S::View,
        W::Anywhere,
        &[with_alt(secondary_shift("h"))],
        true,
    ),
    bind(C::EditDen, "Edit the Den", S::View, W::Anywhere, &[], true),
    bind(
        C::ChooseDen,
        "Choose a den\u{2026}",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::SaveDenAs,
        "Save the den as\u{2026}",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::RenameDen,
        "Rename the den\u{2026}",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::DeleteDen,
        "Delete the den\u{2026}",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::OpenDensFolder,
        "Open dens folder",
        S::View,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::Larger,
        "Larger interface",
        S::View,
        W::Anywhere,
        &[secondary("="), secondary("+")],
        true,
    ),
    bind(
        C::Smaller,
        "Smaller interface",
        S::View,
        W::Anywhere,
        &[secondary("-")],
        true,
    ),
    bind(
        C::ActualSize,
        "Actual interface size",
        S::View,
        W::Anywhere,
        &[secondary("0")],
        true,
    ),
    // Application
    bind(
        C::Shortcuts,
        "Keyboard shortcuts",
        S::Application,
        W::Panes,
        &[key("?")],
        true,
    ),
    bind(
        C::Quit,
        "Quit Leon",
        S::Application,
        W::Anywhere,
        &[mac(secondary("q")), other(secondary_shift("q"))],
        true,
    ),
    bind(
        C::CloseWindow,
        "Close the window",
        S::Application,
        W::Anywhere,
        &[mac(secondary_shift("w"))],
        true,
    ),
    bind(
        C::About,
        "About Leon",
        S::Application,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::CheckForUpdates,
        "Check for updates…",
        S::Application,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::RestartToUpdate,
        "Restart to update",
        S::Application,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::ShowReleaseNotes,
        "Show release notes",
        S::Application,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::SkipVersion,
        "Skip this version",
        S::Application,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::OpenDownloadPage,
        "Open the download page",
        S::Application,
        W::Anywhere,
        &[],
        true,
    ),
    bind(
        C::Close,
        "Close, or go back",
        S::Application,
        W::Anywhere,
        &[key("escape")],
        false,
    ),
];

/// The chords of `binding` that do something on a platform that is, or is
/// not, macOS. Copy and paste only exist inside a terminal, where a plain
/// `Ctrl+C` is the program's: it is not listed for Linux and Windows.
pub fn chords_on(binding: &Binding, mac: bool) -> Vec<Chord> {
    binding
        .chords
        .iter()
        .filter(|chord| exists_on(chord, mac))
        .filter(|chord| binding.when != When::Terminal || kept_in_terminal(binding, chord, mac))
        .copied()
        .collect()
}

/// The binding of a command.
pub fn binding(command: Command) -> Option<&'static Binding> {
    BINDINGS.iter().find(|binding| binding.command == command)
}

/// Words that find a command in the palette besides its name.
pub fn keywords(command: Command) -> &'static str {
    use Command as C;
    match command {
        C::ChooseTheme => "colors colours look appearance custom user",
        C::NewThemeFromCurrent => "create custom make theme colors colours file toml",
        C::AddAgent => "custom cli tool command new agent register any",
        C::RemoveAgent => "custom cli tool delete forget agent unregister",
        C::ExportTheme => "save write copy theme colors colours file toml backup",
        C::OpenThemesFolder => "themes folder directory reveal finder custom files",
        C::ReloadThemes => "refresh themes custom files toml",
        C::ShowThemeProblems => "errors warnings invalid validation themes report debug",
        C::Quit => "exit close application",
        C::CheckForUpdates => "update upgrade new version latest release github",
        C::RestartToUpdate => "update upgrade install relaunch restart new version",
        C::ShowReleaseNotes => "changelog what's new changes update version",
        C::SkipVersion => "ignore dismiss update later version",
        C::OpenDownloadPage => "github releases browser manual update upgrade",
        C::Find => "search terminal scrollback",
        C::ClearBuffer | C::ClearScrollback => "reset empty erase terminal",
        C::PasteText | C::PasteImage => "clipboard image picture screenshot ctrl+v",
        C::SaveOutput | C::SaveOutputAnsi => "export write log terminal file",
        C::OpenFile => "edit editor text code source path open",
        C::SaveFile | C::CloseFile => "edit editor text code source write",
        C::TogglePreview => "markdown md render page split view readme",
        C::RevealInTree => "show select locate explorer sidebar folder",
        C::QuickOpen => "file name fuzzy find go open path goto ctrl+p",
        C::FindInFile | C::ReplaceInFile => "search editor text code substitute change",
        C::SearchProject => "grep ripgrep find text content files everywhere project code",
        C::CopyAll | C::CopyScreen => "clipboard terminal output",
        C::ToggleSidebar => "hide show panel tree",
        C::ToggleFiles => "hide show panel explorer files folder tree project",
        C::AddMachine => {
            "add machine remote server code relay ssh connect computer host login pair"
        }
        C::ShareMachine => "share host pair code relay devices revoke remote access let connect",
        C::EditMachine => "ssh host user port identity key server remote change connect",
        C::WhyOffline => "offline unreachable diagnose test connection ssh remote server",
        C::Settings => "preferences options configuration config",
        C::RestoreSessions => "reopen open terminals tabs panes crash power quit previous session workspace",
        C::WhyMissing => "history sessions lost gone disappeared import diagnose opencode claude codex report",
        C::ShowDen => "lions pride office agents live sessions watch activity pixel narrator who is working",
        C::GoHome => "home start dashboard quickstart overview nothing open back sessions summary",
        C::EditDen => "den furniture move place rotate desk decorate customise customize arrange room layout build",
        C::ChooseDen => "den prefab office library lounge server room nook open plan layout switch pick",
        C::SaveDenAs => "den layout save copy duplicate keep name new",
        C::RenameDen => "den layout name title",
        C::DeleteDen => "den layout remove discard",
        C::OpenDensFolder => "dens folder directory reveal finder files json export import share",
        C::ShowUsage | C::RefreshUsage => "limits quota rate tokens credits remaining percent reset five hour weekly claude codex opencode",
        C::OpenSettingsFile => "preferences json edit configuration config file",
        C::RevealSettingsFolder => "preferences json configuration config finder directory data",
        _ => "",
    }
}

/// Whether a command acts on the file that has the keyboard, and so only
/// applies while one does.
pub fn needs_file(command: Command) -> bool {
    matches!(
        command,
        Command::SaveFile
            | Command::CloseFile
            | Command::FindInFile
            | Command::ReplaceInFile
            | Command::TogglePreview
            | Command::RevealInTree
    )
}

/// What a command is called.
pub fn label(command: Command) -> &'static str {
    binding(command).map_or("", |binding| binding.label)
}

/// The keys shown next to a command, written as this platform does; `None`
/// for a command without keys.
pub fn keys_label(command: Command) -> Option<String> {
    chords_on(binding(command)?, crate::platform::is_mac())
        .first()
        .map(Chord::label)
}

/// What is true of the window when a key is pressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Context {
    /// A text field has the keyboard: a key on its own is a character.
    pub typing: bool,
    /// A terminal has the keyboard: see the module documentation for which
    /// chords stay Leon's.
    pub terminal: bool,
    /// An editor has the keyboard: the chords of [`When::File`] apply.
    pub file: bool,
}

/// Whether `chord` exists on a platform that is, or is not, macOS.
pub fn exists_on(chord: &Chord, mac: bool) -> bool {
    match chord.only {
        Only::Any => true,
        Only::Mac => mac,
        Only::Other => !mac,
    }
}

/// Whether `chord` of `binding` is still Leon's while a terminal has the
/// keyboard, on a platform that is, or is not, macOS. The one place that
/// decides it; the shortcuts sheet and the tests read it too.
pub fn kept_in_terminal(binding: &Binding, chord: &Chord, mac: bool) -> bool {
    match binding.when {
        // A bare key belongs to the program.
        When::Panes => false,
        // Cmd on macOS, Ctrl+Shift elsewhere.
        When::Anywhere => chord.secondary && (mac || chord.shift),
        // An editor is not a terminal: its chords never apply there.
        When::File => false,
        // Scrollback keys are bare. Copy and paste also keep the operating
        // system's ordinary primary-modifier chord on every platform; the
        // shell takes a copy chord only when there is a selection and gives
        // it back to the program when there is not (see `ui/shell.rs`).
        When::Terminal => {
            !chord.secondary
                || mac
                || chord.shift
                || matches!(binding.command, Command::Copy | Command::Paste)
        }
    }
}

/// Every chord that is Leon's while a terminal has the keyboard.
pub fn terminal_chords(mac: bool) -> Vec<(Command, Chord)> {
    BINDINGS
        .iter()
        .flat_map(|binding| {
            binding
                .chords
                .iter()
                .filter(move |chord| exists_on(chord, mac) && kept_in_terminal(binding, chord, mac))
                .map(move |chord| (binding.command, *chord))
        })
        .collect()
}

/// The command a keystroke stands for, given what is true of the window.
pub fn resolve(stroke: &Keystroke, context: Context) -> Option<Command> {
    resolve_on(stroke, context, crate::platform::is_mac())
}

/// [`resolve`] for a platform that is, or is not, macOS.
pub fn resolve_on(stroke: &Keystroke, context: Context, mac: bool) -> Option<Command> {
    BINDINGS
        .iter()
        .filter(|binding| match binding.when {
            When::Anywhere => true,
            When::Panes => !context.typing && !context.terminal,
            When::Terminal => context.terminal,
            When::File => context.file,
        })
        .find(|binding| {
            binding.chords.iter().any(|chord| {
                (!context.terminal || kept_in_terminal(binding, chord, mac))
                    && chord.matches_on(stroke, mac)
            })
        })
        .map(|binding| binding.command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn stroke(source: &str) -> Keystroke {
        Keystroke::parse(source).expect("a keystroke")
    }

    /// A character as the platform reports it when it is typed.
    fn typed(key: &str, character: &str, shift: bool) -> Keystroke {
        let mut stroke = Keystroke::parse(key).expect("a keystroke");
        stroke.modifiers.shift = shift;
        stroke.key_char = Some(character.to_owned());
        stroke
    }

    const NOT_TYPING: Context = Context {
        typing: false,
        terminal: false,
        file: false,
    };
    const TYPING: Context = Context {
        typing: true,
        terminal: false,
        file: false,
    };
    const TERMINAL: Context = Context {
        typing: false,
        terminal: true,
        file: false,
    };
    const FILE: Context = Context {
        typing: true,
        terminal: false,
        file: true,
    };

    /// Every variant, written out. The `match` makes adding a variant without
    /// listing it here a compile error.
    fn every_command() -> Vec<Command> {
        let mut all = vec![
            C::GoTo,
            C::Commands,
            C::SearchHistory,
            C::FilterProjects,
            C::ToggleActiveOnly,
            C::RefreshIcon,
            C::ChooseIcon,
            C::ResetIcon,
            C::FocusSidebar,
            C::FocusMain,
            C::NextPane,
            C::PreviousPane,
            C::Down,
            C::Up,
            C::Top,
            C::Bottom,
            C::PageDown,
            C::PageUp,
            C::Expand,
            C::Collapse,
            C::Open,
            C::NewSession,
            C::AddAgent,
            C::RemoveAgent,
            C::NewWorktree,
            C::ResumeSession,
            C::ResumeIn,
            C::ResumeAnyway,
            C::TakeOver,
            C::RevealTerminal,
            C::OpenShell,
            C::OpenFile,
            C::SaveFile,
            C::CloseFile,
            C::QuickOpen,
            C::FindInFile,
            C::ReplaceInFile,
            C::SearchProject,
            C::TogglePreview,
            C::RevealInTree,
            C::SplitRight,
            C::SplitDown,
            C::FocusPaneLeft,
            C::FocusPaneRight,
            C::FocusPaneUp,
            C::FocusPaneDown,
            C::NextSplit,
            C::PreviousSplit,
            C::ResizeLeft,
            C::ResizeRight,
            C::ResizeUp,
            C::ResizeDown,
            C::EqualizeSplits,
            C::ToggleZoom,
            C::NextTab,
            C::PreviousTab,
            C::Rename,
            C::MoveRowUp,
            C::MoveRowDown,
            C::PinSession,
            C::UnpinSession,
            C::RemoveMachine,
            C::EditMachine,
            C::WhyOffline,
            C::CopyPath,
            C::CopyBranch,
            C::CopySessionId,
            C::Reveal,
            C::OpenTranscript,
            C::RemoveFromHistory,
            C::ContextMenu,
            C::CloseSession,
            C::SleepSession,
            C::FocusTerminal,
            C::Copy,
            C::Paste,
            C::ScrollPageUp,
            C::ScrollPageDown,
            C::AddMachine,
            C::ShareMachine,
            C::OpenProject,
            C::CloneProject,
            C::NewProject,
            C::AddProject,
            C::RemoveProject,
            C::RemoveWorktree,
            C::Refresh,
            C::ProbeMachine,
            C::ToggleAppearance,
            C::SetAppearance,
            C::ChooseTheme,
            C::NextTheme,
            C::PreviousTheme,
            C::SetInterfaceSize,
            C::Settings,
            C::ShowUsage,
            C::RefreshUsage,
            C::ShowDen,
            C::GoHome,
            C::EditDen,
            C::ChooseDen,
            C::SaveDenAs,
            C::RenameDen,
            C::DeleteDen,
            C::OpenDensFolder,
            C::WhyMissing,
            C::RestoreSessions,
            C::OpenSettingsFile,
            C::RevealSettingsFolder,
            C::Larger,
            C::Smaller,
            C::ActualSize,
            C::Shortcuts,
            C::Close,
            C::Quit,
            C::CloseWindow,
            C::About,
            C::CheckForUpdates,
            C::RestartToUpdate,
            C::ShowReleaseNotes,
            C::SkipVersion,
            C::OpenDownloadPage,
            C::Find,
            C::FindNext,
            C::FindPrevious,
            C::ClearBuffer,
            C::ClearScrollback,
            C::CopyAll,
            C::CopyScreen,
            C::SelectAll,
            C::SaveOutput,
            C::SaveOutputAnsi,
            C::PasteText,
            C::PasteImage,
            C::ToggleSidebar,
            C::ToggleFiles,
            C::WidenSidebar,
            C::NarrowSidebar,
            C::ResetSidebarWidth,
            C::NewThemeFromCurrent,
            C::ExportTheme,
            C::OpenThemesFolder,
            C::ReloadThemes,
            C::ShowThemeProblems,
        ];
        all.extend((1..=9).map(C::Machine));
        all.extend((1..=9).map(C::Tab));
        for command in &all {
            match command {
                C::GoTo
                | C::Commands
                | C::SearchHistory
                | C::FilterProjects
                | C::ToggleActiveOnly
                | C::RefreshIcon
                | C::ChooseIcon
                | C::ResetIcon
                | C::Machine(_)
                | C::FocusSidebar
                | C::FocusMain
                | C::NextPane
                | C::PreviousPane
                | C::Down
                | C::Up
                | C::Top
                | C::Bottom
                | C::PageDown
                | C::PageUp
                | C::Expand
                | C::Collapse
                | C::Open
                | C::NewSession
                | C::AddAgent
                | C::RemoveAgent
                | C::NewWorktree
                | C::ResumeSession
                | C::ResumeIn
                | C::ResumeAnyway
                | C::TakeOver
                | C::RevealTerminal
                | C::OpenShell
                | C::OpenFile
                | C::SaveFile
                | C::CloseFile
                | C::QuickOpen
                | C::FindInFile
                | C::ReplaceInFile
                | C::SearchProject
                | C::TogglePreview
                | C::RevealInTree
                | C::SplitRight
                | C::SplitDown
                | C::FocusPaneLeft
                | C::FocusPaneRight
                | C::FocusPaneUp
                | C::FocusPaneDown
                | C::NextSplit
                | C::PreviousSplit
                | C::ResizeLeft
                | C::ResizeRight
                | C::ResizeUp
                | C::ResizeDown
                | C::EqualizeSplits
                | C::ToggleZoom
                | C::NextTab
                | C::PreviousTab
                | C::Tab(_)
                | C::Rename
                | C::MoveRowUp
                | C::MoveRowDown
                | C::PinSession
                | C::UnpinSession
                | C::RemoveMachine
                | C::EditMachine
                | C::WhyOffline
                | C::CopyPath
                | C::CopyBranch
                | C::CopySessionId
                | C::Reveal
                | C::OpenTranscript
                | C::RemoveFromHistory
                | C::ContextMenu
                | C::CloseSession
                | C::SleepSession
                | C::FocusTerminal
                | C::Copy
                | C::Paste
                | C::ScrollPageUp
                | C::ScrollPageDown
                | C::AddMachine
                | C::ShareMachine
                | C::OpenProject
                | C::CloneProject
                | C::NewProject
                | C::AddProject
                | C::RemoveProject
                | C::RemoveWorktree
                | C::Refresh
                | C::ProbeMachine
                | C::ToggleAppearance
                | C::SetAppearance
                | C::ChooseTheme
                | C::NextTheme
                | C::PreviousTheme
                | C::SetInterfaceSize
                | C::Settings
                | C::ShowUsage
                | C::RefreshUsage
                | C::ShowDen
                | C::GoHome
                | C::EditDen
                | C::ChooseDen
                | C::SaveDenAs
                | C::RenameDen
                | C::DeleteDen
                | C::OpenDensFolder
                | C::WhyMissing
                | C::RestoreSessions
                | C::OpenSettingsFile
                | C::RevealSettingsFolder
                | C::Larger
                | C::Smaller
                | C::ActualSize
                | C::Shortcuts
                | C::Close
                | C::Quit
                | C::CloseWindow
                | C::About
                | C::CheckForUpdates
                | C::RestartToUpdate
                | C::ShowReleaseNotes
                | C::SkipVersion
                | C::OpenDownloadPage
                | C::Find
                | C::FindNext
                | C::FindPrevious
                | C::ClearBuffer
                | C::ClearScrollback
                | C::CopyAll
                | C::CopyScreen
                | C::SelectAll
                | C::SaveOutput
                | C::SaveOutputAnsi
                | C::PasteText
                | C::PasteImage
                | C::ToggleSidebar
                | C::ToggleFiles
                | C::WidenSidebar
                | C::NarrowSidebar
                | C::ResetSidebarWidth
                | C::NewThemeFromCurrent
                | C::ExportTheme
                | C::OpenThemesFolder
                | C::ReloadThemes
                | C::ShowThemeProblems => {}
            }
        }
        all
    }

    #[test]
    fn every_command_is_bound_once_and_named() {
        let mut seen = HashSet::new();
        for binding in BINDINGS {
            assert!(
                seen.insert(binding.command),
                "{:?} is listed twice",
                binding.command
            );
            assert!(
                !binding.label.trim().is_empty(),
                "{:?} has no name",
                binding.command
            );
            assert!(Section::ALL.contains(&binding.section));
        }
        for command in every_command() {
            assert!(seen.contains(&command), "{command:?} is not in BINDINGS");
        }
        assert_eq!(seen.len(), every_command().len());
    }

    #[test]
    fn no_two_commands_share_a_chord() {
        // A chord of "anywhere" applies in every context, so a chord can
        // belong to one command only, whatever the contexts.
        // Checked for each platform: a chord only for macOS may share its
        // keys with one only for Linux.
        for mac in [true, false] {
            type Taken<'a> = HashMap<(&'a str, bool, bool, bool, bool), (Command, When)>;
            let mut taken: Taken = HashMap::new();
            for binding in BINDINGS {
                for chord in chords_on(binding, true)
                    .into_iter()
                    .filter(|_| mac)
                    .chain(chords_on(binding, false).into_iter().filter(|_| !mac))
                {
                    let id = (
                        chord.key,
                        chord.secondary,
                        chord.shift,
                        chord.alt,
                        chord.control,
                    );
                    if let Some((other, other_when)) =
                        taken.insert(id, (binding.command, binding.when))
                    {
                        // The one deliberate sharing: a chord that means
                        // something only while a terminal or an editor has
                        // the keyboard, listed before (so winning over) the
                        // command it shares the chord with everywhere else.
                        let deliberate = matches!(other_when, When::Terminal | When::File)
                            && binding.when != other_when
                            && matches!(
                                (other, binding.command),
                                (C::ClearBuffer, C::GoTo)
                                    | (C::Find, C::FindInFile)
                                    | (C::Find, C::FilterProjects)
                                    | (C::FindInFile, C::FilterProjects)
                                    | (C::SaveOutput, C::SaveFile)
                            );
                        assert!(
                            deliberate,
                            "{} is both {other:?} and {:?} (mac: {mac})",
                            chord.label_for(mac),
                            binding.command
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_bare_key_is_never_anywhere_except_escape() {
        // Anywhere includes text fields: a bare key there would eat typing.
        for binding in BINDINGS.iter().filter(|b| b.when == When::Anywhere) {
            for chord in binding.chords {
                assert!(
                    chord.secondary || chord.key == "escape",
                    "{:?}: {} would swallow typing",
                    binding.command,
                    chord.label_for(false)
                );
            }
        }
    }

    #[test]
    fn the_required_shortcuts_are_in_the_table() {
        let chord = |command| binding(command).unwrap().chords[0];
        assert_eq!(chord(C::GoTo), secondary("p"));
        assert_eq!(binding(C::GoTo).unwrap().chords[1], secondary("k"));
        assert_eq!(chord(C::Commands), secondary_shift("p"));
        assert_eq!(chord(C::SearchHistory), mac(secondary_shift("f")));
        assert_eq!(chord(C::ToggleSidebar), mac(secondary("b")));
        assert_eq!(chord(C::FocusSidebar), secondary("l"));
        assert_eq!(chord(C::OpenProject), secondary("o"));
        assert_eq!(chord(C::NewSession), secondary("n"));
        assert_eq!(chord(C::NewWorktree), secondary_shift("n"));
        assert_eq!(chord(C::Shortcuts), key("?"));
        assert_eq!(chord(C::Close), key("escape"));
        for n in 1..=9u8 {
            let digit = ["1", "2", "3", "4", "5", "6", "7", "8", "9"][usize::from(n) - 1];
            assert_eq!(chord(C::Machine(n)), secondary(digit));
        }
    }

    #[test]
    fn every_palette_command_has_a_name_and_the_sheet_lists_every_binding() {
        for binding in BINDINGS.iter().filter(|b| b.palette) {
            assert!(!binding.label.is_empty());
        }
        let in_sheet: usize = Section::ALL
            .iter()
            .map(|section| BINDINGS.iter().filter(|b| b.section == *section).count())
            .sum();
        assert_eq!(in_sheet, BINDINGS.len(), "every binding is in a section");
    }

    #[test]
    fn chords_are_labelled_the_way_each_platform_writes_them() {
        assert_eq!(secondary("k").label_for(true), "⌘K");
        assert_eq!(secondary("k").label_for(false), "Ctrl+K");
        assert_eq!(secondary_shift("p").label_for(true), "⇧⌘P");
        assert_eq!(secondary_shift("p").label_for(false), "Ctrl+Shift+P");
        assert_eq!(key("escape").label_for(true), "Esc");
        assert_eq!(key("enter").label_for(true), "↩");
        assert_eq!(key("enter").label_for(false), "Enter");
        assert_eq!(key("?").label_for(false), "?");
        assert_eq!(shift("tab").label_for(false), "Shift+Tab");
        assert_eq!(key("down").label_for(false), "↓");
    }

    #[test]
    fn the_secondary_key_is_cmd_on_macos_and_ctrl_elsewhere() {
        assert_eq!(
            resolve_on(&stroke("cmd-p"), NOT_TYPING, true),
            Some(C::GoTo)
        );
        assert_eq!(resolve_on(&stroke("ctrl-p"), NOT_TYPING, true), None);
        assert_eq!(
            resolve_on(&stroke("ctrl-p"), NOT_TYPING, false),
            Some(C::GoTo)
        );
        assert_eq!(resolve_on(&stroke("cmd-p"), NOT_TYPING, false), None);
    }

    #[test]
    fn shift_tells_the_two_palettes_apart() {
        for mac in [true, false] {
            let held = if mac { "cmd" } else { "ctrl" };
            assert_eq!(
                resolve_on(&stroke(&format!("{held}-k")), TYPING, mac),
                Some(C::GoTo)
            );
            assert_eq!(
                resolve_on(&stroke(&format!("{held}-shift-p")), TYPING, mac),
                Some(C::Commands)
            );
            assert_eq!(
                resolve_on(&stroke(&format!("{held}-shift-n")), TYPING, mac),
                Some(C::NewWorktree)
            );
            assert_eq!(
                resolve_on(&stroke(&format!("{held}-n")), TYPING, mac),
                Some(C::NewSession)
            );
            assert_eq!(
                resolve_on(&stroke(&format!("{held}-5")), TYPING, mac),
                Some(C::Machine(5))
            );
        }
    }

    #[test]
    fn a_bare_key_is_a_command_only_while_nothing_is_being_typed() {
        assert_eq!(
            resolve_on(&typed("j", "j", false), NOT_TYPING, false),
            Some(C::Down)
        );
        assert_eq!(resolve_on(&typed("j", "j", false), TYPING, false), None);
        assert_eq!(
            resolve_on(&stroke("down"), NOT_TYPING, false),
            Some(C::Down)
        );
        assert_eq!(
            resolve_on(&stroke("enter"), NOT_TYPING, false),
            Some(C::Open)
        );
        assert_eq!(
            resolve_on(&stroke("tab"), NOT_TYPING, false),
            Some(C::NextPane)
        );
        assert_eq!(
            resolve_on(&stroke("shift-tab"), NOT_TYPING, false),
            Some(C::PreviousPane)
        );
    }

    #[test]
    fn the_tree_keys_expand_collapse_and_open_only_while_nothing_is_being_typed() {
        for (key, command) in [
            ("l", C::Expand),
            ("right", C::Expand),
            ("h", C::Collapse),
            ("left", C::Collapse),
            ("enter", C::Open),
        ] {
            assert_eq!(
                resolve_on(&typed(key, key, false), NOT_TYPING, false),
                Some(command),
                "{key}"
            );
            assert_eq!(resolve_on(&typed(key, key, false), TYPING, false), None);
        }
    }

    #[test]
    fn the_sidebar_has_one_focus_chord_and_an_alias_for_the_old_list_one() {
        assert_eq!(
            resolve_on(&stroke("ctrl-shift-s"), NOT_TYPING, false),
            Some(C::FocusSidebar)
        );
        assert_eq!(
            resolve_on(&stroke("ctrl-b"), NOT_TYPING, false),
            None,
            "a plain Ctrl+B is the program's (tmux)"
        );
        assert_eq!(
            resolve_on(&stroke("ctrl-l"), TYPING, false),
            Some(C::FocusSidebar)
        );
    }

    #[test]
    fn opening_a_project_is_secondary_o_and_is_listed_in_the_palette_as_open_project() {
        assert_eq!(
            resolve_on(&stroke("cmd-o"), TYPING, true),
            Some(C::OpenProject)
        );
        let open = binding(C::OpenProject).unwrap();
        assert_eq!(open.label, "Open project…");
        assert!(open.palette);
        assert_eq!(binding(C::AddMachine).unwrap().label, "Connect a machine…");
        assert_eq!(
            keys_label(C::OpenProject).as_deref(),
            Some(secondary("o").label().as_str())
        );
    }

    #[test]
    fn escape_works_while_typing() {
        assert_eq!(resolve_on(&stroke("escape"), TYPING, false), Some(C::Close));
        assert_eq!(
            resolve_on(&stroke("escape"), NOT_TYPING, false),
            Some(C::Close)
        );
    }

    #[test]
    fn the_question_mark_is_the_character_typed_on_any_layout() {
        assert_eq!(
            resolve_on(&typed("/", "?", true), NOT_TYPING, false),
            Some(C::Shortcuts)
        );
        assert_eq!(resolve_on(&typed("/", "?", true), TYPING, false), None);
        // A plain slash is the filter's key, and only where nothing is typed.
        assert_eq!(
            resolve_on(&typed("/", "/", false), NOT_TYPING, false),
            Some(C::FilterProjects)
        );
        assert_eq!(resolve_on(&typed("/", "/", false), TYPING, false), None);
    }

    #[test]
    fn the_slash_is_the_filters_key_on_layouts_that_type_it_with_shift() {
        // Spanish, German, Italian...: `/` is Shift+7, so the key is `7`, Shift
        // is held, and what was typed is `/`.
        assert_eq!(
            resolve_on(&typed("7", "/", true), NOT_TYPING, false),
            Some(C::FilterProjects)
        );
        assert_eq!(
            resolve_on(&typed("7", "/", true), NOT_TYPING, true),
            Some(C::FilterProjects)
        );
        // Typing in a field or a terminal still gets the character.
        assert_eq!(resolve_on(&typed("7", "/", true), TYPING, false), None);
        assert_eq!(resolve_on(&typed("7", "/", true), TERMINAL, false), None);
        // Shift+/ on a US layout is `?`, not the filter.
        assert_eq!(
            resolve_on(&typed("/", "?", true), NOT_TYPING, false),
            Some(C::Shortcuts)
        );
        // Without a typed character the key's name decides, and Shift+/ is `?`.
        let mut bare = Keystroke::parse("shift-/").unwrap();
        bare.key_char = None;
        assert_ne!(
            resolve_on(&bare, NOT_TYPING, false),
            Some(C::FilterProjects)
        );
    }

    #[test]
    fn a_capital_g_goes_to_the_bottom_and_a_small_g_to_the_top() {
        assert_eq!(
            resolve_on(&typed("g", "g", false), NOT_TYPING, false),
            Some(C::Top)
        );
        assert_eq!(
            resolve_on(&typed("g", "G", true), NOT_TYPING, false),
            Some(C::Bottom)
        );
    }

    #[test]
    fn plus_and_equals_both_make_the_interface_larger() {
        assert_eq!(
            resolve_on(&stroke("ctrl-="), TYPING, false),
            Some(C::Larger)
        );
        assert_eq!(
            resolve_on(&stroke("ctrl-shift-="), TYPING, false),
            Some(C::Larger)
        );
    }

    #[test]
    fn alt_and_function_modified_keys_are_not_commands() {
        assert_eq!(resolve_on(&stroke("alt-j"), NOT_TYPING, false), None);
        assert_eq!(resolve_on(&stroke("ctrl-alt-p"), NOT_TYPING, false), None);
    }

    #[test]
    fn a_terminal_program_receives_non_editing_ctrl_chords_escape_tab_and_the_arrows() {
        for mac in [true, false] {
            // Ctrl is the program's on both: Cmd, not Ctrl, is macOS's secondary.
            let held = "ctrl";
            for key in [
                &format!("{held}-d"),
                &format!("{held}-r"),
                &format!("{held}-l"),
                &format!("{held}-z"),
                "escape",
                "tab",
                "shift-tab",
                "up",
                "down",
                "left",
                "right",
                "enter",
                "pageup",
                "j",
                "?",
            ] {
                assert_eq!(
                    resolve_on(&stroke(key), TERMINAL, mac),
                    None,
                    "{key} must reach the program (mac: {mac})"
                );
            }
        }
    }

    #[test]
    fn on_macos_every_cmd_chord_stays_leons_in_a_terminal() {
        for binding in BINDINGS.iter().filter(|b| b.when == When::Anywhere) {
            for chord in binding.chords.iter().filter(|chord| chord.secondary) {
                assert!(
                    kept_in_terminal(binding, chord, true),
                    "{:?} {}",
                    binding.command,
                    chord.label_for(true)
                );
            }
        }
        // The only bare chord of an "anywhere" binding is escape, and it is
        // the program's.
        assert_eq!(resolve_on(&stroke("escape"), TERMINAL, true), None);
        assert_eq!(
            resolve_on(&stroke("cmd-n"), TERMINAL, true),
            Some(C::NewSession)
        );
        assert_eq!(resolve_on(&stroke("cmd-c"), TERMINAL, true), Some(C::Copy));
        assert_eq!(resolve_on(&stroke("cmd-v"), TERMINAL, true), Some(C::Paste));
        assert_eq!(
            resolve_on(&stroke("cmd-w"), TERMINAL, true),
            Some(C::CloseSession)
        );
    }

    #[test]
    fn off_macos_native_copy_paste_and_ctrl_shift_chords_stay_leons_in_a_terminal() {
        for (command, chord) in terminal_chords(false) {
            assert!(
                chord.shift || !chord.secondary || matches!(command, C::Copy | C::Paste),
                "{command:?} {} would steal a Ctrl chord from the program",
                chord.label_for(false)
            );
        }
        for (key, command) in [
            ("ctrl-shift-p", C::Commands),
            ("ctrl-shift-i", C::SearchHistory),
            ("ctrl-shift-f", C::Find),
            ("ctrl-shift-e", C::FindNext),
            ("ctrl-shift-u", C::FindPrevious),
            ("ctrl-shift-x", C::ClearBuffer),
            ("ctrl-shift-z", C::ClearScrollback),
            ("ctrl-shift-q", C::Quit),
            ("ctrl-shift-k", C::GoTo),
            ("ctrl-shift-b", C::ToggleSidebar),
            ("ctrl-shift-s", C::FocusSidebar),
            ("ctrl-shift-a", C::NewSession),
            ("ctrl-shift-t", C::OpenShell),
            ("ctrl-shift-w", C::CloseSession),
            ("ctrl-shift-d", C::SplitRight),
            ("ctrl-shift-o", C::SplitDown),
            ("ctrl-shift-]", C::NextSplit),
            ("ctrl-shift-[", C::PreviousSplit),
            ("ctrl-shift-pagedown", C::NextTab),
            ("ctrl-shift-pageup", C::PreviousTab),
            ("ctrl-shift-left", C::FocusPaneLeft),
            ("ctrl-shift-up", C::FocusPaneUp),
            ("ctrl-shift-alt-left", C::ResizeLeft),
            ("ctrl-shift-alt-down", C::ResizeDown),
            ("ctrl-shift-enter", C::ToggleZoom),
            ("ctrl-shift-g", C::EqualizeSplits),
            ("ctrl-shift-3", C::Tab(3)),
            ("ctrl-c", C::Copy),
            ("ctrl-shift-c", C::Copy),
            ("ctrl-v", C::Paste),
            ("ctrl-shift-v", C::Paste),
            ("shift-pageup", C::ScrollPageUp),
            ("shift-pagedown", C::ScrollPageDown),
        ] {
            assert_eq!(
                resolve_on(&stroke(key), TERMINAL, false),
                Some(command),
                "{key}"
            );
        }
        for key in [
            "ctrl-p", "ctrl-n", "ctrl-w", "ctrl-t", "ctrl-1", "ctrl-b", "ctrl-o",
        ] {
            assert_eq!(resolve_on(&stroke(key), TERMINAL, false), None, "{key}");
        }
    }

    #[test]
    fn the_theme_chords_are_free_and_stay_leons_in_a_terminal_on_every_platform() {
        for mac in [true, false] {
            let (next, previous) = if mac {
                ("cmd-shift-j", "cmd-shift-h")
            } else {
                ("ctrl-shift-j", "ctrl-shift-h")
            };
            for context in [NOT_TYPING, TYPING, TERMINAL] {
                assert_eq!(
                    resolve_on(&stroke(next), context, mac),
                    Some(C::NextTheme),
                    "{next} mac={mac}"
                );
                assert_eq!(
                    resolve_on(&stroke(previous), context, mac),
                    Some(C::PreviousTheme),
                    "{previous} mac={mac}"
                );
            }
        }
        for command in [C::NextTheme, C::PreviousTheme] {
            let binding = binding(command).unwrap();
            for mac in [true, false] {
                for chord in chords_on(binding, mac) {
                    assert!(kept_in_terminal(binding, &chord, mac));
                }
            }
        }
    }

    #[test]
    fn copy_paste_and_scrollback_only_exist_while_a_terminal_has_the_keyboard() {
        for key in ["ctrl-c", "ctrl-shift-c", "ctrl-v", "shift-pageup"] {
            assert_eq!(resolve_on(&stroke(key), NOT_TYPING, false), None, "{key}");
            assert_eq!(resolve_on(&stroke(key), TYPING, false), None, "{key}");
        }
        assert_eq!(resolve_on(&stroke("cmd-c"), TYPING, true), None);
    }

    #[test]
    fn the_tree_keys_do_not_apply_while_a_terminal_has_the_keyboard() {
        assert_eq!(resolve_on(&stroke("down"), TERMINAL, false), None);
        assert_eq!(resolve_on(&typed("j", "j", false), TERMINAL, false), None);
    }

    #[test]
    fn every_command_that_matters_in_a_terminal_has_a_chord_that_stays_leons_on_every_platform() {
        for command in [
            C::Commands,
            C::GoTo,
            C::FocusSidebar,
            C::NewSession,
            C::OpenShell,
            C::OpenTranscript,
            C::CloseSession,
            C::SplitRight,
            C::SplitDown,
            C::FocusPaneLeft,
            C::FocusPaneRight,
            C::FocusPaneUp,
            C::FocusPaneDown,
            C::NextSplit,
            C::PreviousSplit,
            C::ResizeLeft,
            C::ResizeRight,
            C::ResizeUp,
            C::ResizeDown,
            C::EqualizeSplits,
            C::ToggleZoom,
            C::NextTab,
            C::PreviousTab,
            C::Copy,
            C::Paste,
        ] {
            for mac in [true, false] {
                assert!(
                    terminal_chords(mac).iter().any(|(c, _)| *c == command),
                    "{command:?} cannot be reached from a terminal (mac: {mac})"
                );
            }
        }
    }

    #[test]
    fn the_new_terminal_shortcuts_are_in_the_table() {
        let first = |command| binding(command).unwrap().chords[0];
        assert_eq!(first(C::OpenShell), secondary("t"));
        assert_eq!(first(C::OpenTranscript), secondary_shift("l"));
        assert_eq!(binding(C::ResumeIn).unwrap().chords.len(), 0);
        assert_eq!(first(C::CloseSession), mac(secondary("w")));
        assert_eq!(first(C::FocusTerminal), secondary("e"));
        assert_eq!(first(C::Copy), secondary("c"));
        assert_eq!(first(C::Paste), secondary("v"));
        assert_eq!(first(C::ScrollPageUp), shift("pageup"));
        assert_eq!(first(C::ScrollPageDown), shift("pagedown"));
    }

    #[test]
    fn enter_opens_the_row_and_the_old_resume_chord_is_free() {
        // Opening a session resumes it, so there is no second command for it:
        // the chord it had goes to nothing, in a terminal as everywhere.
        for mac in [true, false] {
            assert_eq!(resolve_on(&stroke("enter"), NOT_TYPING, mac), Some(C::Open));
            assert_eq!(resolve_on(&stroke("ctrl-enter"), NOT_TYPING, mac), None);
            assert_eq!(resolve_on(&stroke("cmd-enter"), NOT_TYPING, mac), None);
        }
    }

    #[test]
    fn the_transcript_chord_is_free_and_never_the_programs_in_a_terminal() {
        let chord = secondary_shift("l");
        for (other, name) in BINDINGS
            .iter()
            .filter(|binding| binding.command != C::OpenTranscript)
            .map(|binding| (binding, binding.label))
        {
            assert!(
                !other.chords.contains(&chord),
                "{name} already has {}",
                chord.label_for(true)
            );
        }
        assert_eq!(
            resolve_on(&stroke("cmd-shift-l"), TERMINAL, true),
            Some(C::OpenTranscript)
        );
        assert_eq!(
            resolve_on(&stroke("ctrl-shift-l"), TERMINAL, false),
            Some(C::OpenTranscript)
        );
    }

    /// Prints the shortcut tables of the README. Run with
    /// `cargo test -p leon print_readme_tables -- --ignored --nocapture`.
    #[test]
    #[ignore = "prints the README's tables; not a check"]
    fn print_readme_tables() {
        let mut out = String::new();
        out.push_str("| Command | macOS | Linux and Windows |\n| --- | --- | --- |\n");
        for section in Section::ALL {
            out.push_str(&format!("| **{}** | | |\n", section.title()));
            for binding in BINDINGS.iter().filter(|b| b.section == section) {
                if matches!(binding.command, C::Machine(n) if n > 1) {
                    continue;
                }
                let label = if binding.command == C::Machine(1) {
                    "Jump to machine 1 to 9".to_owned()
                } else {
                    binding.label.to_owned()
                };
                let keys = |mac: bool| {
                    let list: Vec<String> = chords_on(binding, mac)
                        .iter()
                        .map(|chord| format!("`{}`", chord.label_for(mac)))
                        .collect();
                    if list.is_empty() {
                        "palette only".to_owned()
                    } else {
                        list.join(" ")
                    }
                };
                out.push_str(&format!("| {label} | {} | {} |\n", keys(true), keys(false)));
            }
        }
        out.push_str("\n### Kept while a terminal has the keyboard\n\n");
        for mac in [true, false] {
            out.push_str(if mac {
                "macOS:\n"
            } else {
                "\nLinux and Windows:\n"
            });
            for (command, chord) in terminal_chords(mac) {
                out.push_str(&format!(
                    "- {} `{}`\n",
                    label(command),
                    chord.label_for(mac)
                ));
            }
        }
        println!("{out}");
    }

    #[test]
    fn the_readme_lists_every_shortcut_for_both_platforms() {
        let readme = include_str!("../../../README.md");
        for binding in BINDINGS {
            if matches!(binding.command, C::Machine(n) if n > 1) {
                continue;
            }
            let label = if binding.command == C::Machine(1) {
                "Jump to machine 1 to 9"
            } else {
                binding.label
            };
            assert!(readme.contains(label), "README lacks {label:?}");
            for mac in [true, false] {
                for chord in chords_on(binding, mac) {
                    let text = format!("`{}`", chord.label_for(mac));
                    assert!(readme.contains(&text), "README lacks {text} for {label:?}");
                }
            }
        }
    }

    #[test]
    fn the_iterm_chords_on_macos() {
        for (key, command) in [
            ("cmd-d", C::SplitRight),
            ("cmd-shift-d", C::SplitDown),
            ("cmd-alt-left", C::FocusPaneLeft),
            ("cmd-alt-right", C::FocusPaneRight),
            ("cmd-alt-up", C::FocusPaneUp),
            ("cmd-alt-down", C::FocusPaneDown),
            ("cmd-]", C::NextSplit),
            ("cmd-[", C::PreviousSplit),
            ("ctrl-cmd-left", C::ResizeLeft),
            ("ctrl-cmd-right", C::ResizeRight),
            ("ctrl-cmd-up", C::ResizeUp),
            ("ctrl-cmd-down", C::ResizeDown),
            ("cmd-shift-enter", C::ToggleZoom),
            ("cmd-w", C::CloseSession),
            ("cmd-t", C::OpenShell),
            ("cmd-shift-]", C::NextTab),
            ("cmd-shift-[", C::PreviousTab),
            ("cmd-alt-1", C::Tab(1)),
            ("cmd-alt-9", C::Tab(9)),
            ("ctrl-cmd-=", C::EqualizeSplits),
        ] {
            assert_eq!(
                resolve_on(&stroke(key), TERMINAL, true),
                Some(command),
                "{key}"
            );
        }
    }

    #[test]
    fn the_tab_chords_do_not_collide_with_the_machine_chords() {
        // Machines keep secondary+1..9; tabs are secondary+alt+n on macOS and
        // ctrl+shift+n elsewhere.
        for (mac, machine, tab) in [
            (true, "cmd-2", "cmd-alt-2"),
            (false, "ctrl-2", "ctrl-shift-2"),
        ] {
            assert_eq!(
                resolve_on(&stroke(machine), NOT_TYPING, mac),
                Some(C::Machine(2))
            );
            assert_eq!(resolve_on(&stroke(tab), NOT_TYPING, mac), Some(C::Tab(2)));
        }
    }

    #[test]
    fn off_macos_the_programs_ctrl_chords_are_never_pane_chords() {
        for key in [
            "ctrl-d",
            "ctrl-w",
            "ctrl-t",
            "ctrl-]",
            "ctrl-left",
            "ctrl-alt-left",
        ] {
            assert_eq!(resolve_on(&stroke(key), TERMINAL, false), None, "{key}");
        }
        // And a macOS-only chord is not one elsewhere, nor the reverse.
        assert_eq!(resolve_on(&stroke("ctrl-cmd-left"), TERMINAL, false), None);
        assert_eq!(resolve_on(&stroke("ctrl-shift-d"), TERMINAL, true), None);
    }

    #[test]
    fn the_file_chords_never_take_a_key_the_program_in_a_terminal_needs() {
        // Off macOS a plain Ctrl+S is the program's (it stops the output):
        // only the file on screen has it.
        assert!(!terminal_chords(false)
            .iter()
            .any(|(command, _)| *command == C::SaveFile));
        assert_eq!(
            resolve_on(&stroke("ctrl-s"), NOT_TYPING, false),
            Some(C::SaveFile)
        );
        assert_eq!(resolve_on(&stroke("ctrl-s"), TERMINAL, false), None);
        // On macOS Cmd+S saves the terminal's output while a terminal has
        // the keyboard, and the file when it is not one.
        assert_eq!(
            resolve_on(&stroke("cmd-s"), TERMINAL, true),
            Some(C::SaveOutput)
        );
        assert_eq!(
            resolve_on(&stroke("cmd-s"), TYPING, true),
            Some(C::SaveFile)
        );
        // Opening a file works from a terminal on every platform.
        for mac in [true, false] {
            assert!(
                terminal_chords(mac)
                    .iter()
                    .any(|(command, _)| *command == C::OpenFile),
                "mac: {mac}"
            );
        }
        assert_eq!(
            resolve_on(&stroke("ctrl-shift-alt-o"), TERMINAL, false),
            Some(C::OpenFile)
        );
        assert_eq!(
            resolve_on(&stroke("cmd-shift-o"), TERMINAL, true),
            Some(C::OpenFile)
        );
        // Closing a file is the pane's chord; the command is the palette's.
        assert!(binding(C::CloseFile).is_some_and(|b| b.chords.is_empty()));
        assert!(needs_file(C::SaveFile) && needs_file(C::CloseFile) && !needs_file(C::OpenFile));
    }

    #[test]
    fn find_in_the_file_is_the_editors_while_a_file_has_the_keyboard_and_nothing_else() {
        // With a file: Cmd+F on macOS, Ctrl+F elsewhere, and replace beside it.
        assert_eq!(
            resolve_on(&stroke("cmd-f"), FILE, true),
            Some(C::FindInFile)
        );
        assert_eq!(
            resolve_on(&stroke("ctrl-f"), FILE, false),
            Some(C::FindInFile)
        );
        assert_eq!(
            resolve_on(&stroke("cmd-alt-f"), FILE, true),
            Some(C::ReplaceInFile)
        );
        assert_eq!(
            resolve_on(&stroke("ctrl-h"), FILE, false),
            Some(C::ReplaceInFile)
        );
        // Without one the chords keep their older meanings: the filter of the
        // sidebar, the terminal's own find, and the program's Ctrl+F.
        assert_eq!(
            resolve_on(&stroke("cmd-f"), NOT_TYPING, true),
            Some(C::FilterProjects)
        );
        assert_eq!(resolve_on(&stroke("cmd-f"), TYPING, true), None);
        assert_eq!(resolve_on(&stroke("ctrl-f"), TYPING, false), None);
        assert_eq!(resolve_on(&stroke("cmd-f"), TERMINAL, true), Some(C::Find));
        assert_eq!(resolve_on(&stroke("ctrl-f"), TERMINAL, false), None);
        assert_eq!(
            resolve_on(&stroke("ctrl-shift-f"), TERMINAL, false),
            Some(C::Find)
        );
        // The commands apply only to a file.
        assert!(needs_file(C::FindInFile) && needs_file(C::ReplaceInFile));
        assert!(!needs_file(C::QuickOpen) && !needs_file(C::SearchProject));
        assert!(!terminal_chords(true)
            .iter()
            .chain(terminal_chords(false).iter())
            .any(|(command, _)| matches!(command, C::FindInFile | C::ReplaceInFile)));
    }

    #[test]
    fn quick_open_and_the_project_search_have_chords_of_their_own_everywhere() {
        // Cmd or Ctrl with P is the palette's "go to": quick open takes the
        // Alt variant.
        assert_eq!(binding(C::GoTo).unwrap().chords[0], secondary("p"));
        for (mac, key) in [(true, "cmd-alt-p"), (false, "ctrl-shift-alt-p")] {
            for context in [NOT_TYPING, TYPING, TERMINAL, FILE] {
                assert_eq!(
                    resolve_on(&stroke(key), context, mac),
                    Some(C::QuickOpen),
                    "{key} {context:?}"
                );
            }
        }
        for (mac, key) in [(true, "cmd-shift-alt-f"), (false, "ctrl-shift-alt-f")] {
            for context in [NOT_TYPING, TYPING, TERMINAL, FILE] {
                assert_eq!(
                    resolve_on(&stroke(key), context, mac),
                    Some(C::SearchProject),
                    "{key} {context:?}"
                );
            }
        }
        // The session history keeps Cmd+Shift+F.
        assert_eq!(
            resolve_on(&stroke("cmd-shift-f"), NOT_TYPING, true),
            Some(C::SearchHistory)
        );
    }

    #[test]
    fn every_pane_command_is_reachable_from_a_terminal_on_every_platform() {
        for command in [
            C::SplitRight,
            C::SplitDown,
            C::FocusPaneLeft,
            C::FocusPaneRight,
            C::FocusPaneUp,
            C::FocusPaneDown,
            C::NextSplit,
            C::PreviousSplit,
            C::ResizeLeft,
            C::ResizeRight,
            C::ResizeUp,
            C::ResizeDown,
            C::EqualizeSplits,
            C::ToggleZoom,
            C::CloseSession,
            C::OpenShell,
            C::NextTab,
            C::PreviousTab,
            C::Tab(1),
            C::Tab(9),
        ] {
            for mac in [true, false] {
                assert!(
                    terminal_chords(mac).iter().any(|(c, _)| *c == command),
                    "{command:?} (mac: {mac})"
                );
            }
        }
    }

    #[test]
    fn the_context_menu_has_its_three_keys_while_the_tree_has_the_keyboard() {
        assert_eq!(
            resolve_on(&stroke("shift-f10"), NOT_TYPING, false),
            Some(C::ContextMenu)
        );
        assert_eq!(
            resolve_on(&stroke("menu"), NOT_TYPING, false),
            Some(C::ContextMenu)
        );
        assert_eq!(
            resolve_on(&typed("m", "m", false), NOT_TYPING, false),
            Some(C::ContextMenu)
        );
        assert_eq!(resolve_on(&typed("m", "m", false), TYPING, false), None);
        assert_eq!(resolve_on(&typed("m", "m", false), TERMINAL, false), None);
        assert_eq!(
            resolve_on(&stroke("f2"), NOT_TYPING, false),
            Some(C::Rename)
        );
    }
}
