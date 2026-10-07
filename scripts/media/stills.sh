#!/usr/bin/env bash
# Every still image of the README, in one clean pass, at 1600x1000.
#
#   scripts/media/nested.sh     once per session
#   scripts/media/stills.sh
set -u
MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)

bash "$ROOT/scripts/media/launch.sh" --reseed >/dev/null
# shellcheck source=/dev/null
. "$ROOT/scripts/media/shots.sh"

# The tree and a project's detail.
goto "atlas"
pause 1.0
shot 01-tree.png

# A live Claude Code session, started through the UI, mid-work and done.
goto "login-timeout"
pause 0.8
combo ctrl shift a
pause 1.4
k Return
pause 4.2
nudge 1.7
shot 02-session-working.png
pause 2.6
shot 03-session-done.png

# A history session resumed in a terminal, and its stored transcript.
goto "close idle"
pause 4.6
combo ctrl shift l
pause 1.6
shot 04-transcript.png
k Escape
pause 0.5

# The command palette.
combo ctrl shift p
pause 0.6
text "theme"
pause 0.8
shot 05-palette.png
k Escape
pause 0.5

# The history search.
combo ctrl shift i
pause 0.8
text "token bucket"
pause 1.2
shot 06-search.png
k Escape
pause 0.5

# Connecting a machine.
combo ctrl shift m
pause 1.2
shot 07-connect.png
k Escape
pause 0.5

# Settings.
ctrl comma
pause 1.2
shot 08-settings.png
k Escape
pause 0.5

# Light and dark.
combo ctrl shift y
pause 1.2
shot 09-light.png
combo ctrl shift y
pause 0.8

# The keyboard shortcuts sheet.
text "?"
pause 1.2
shot 10-shortcuts.png
k Escape
pause 0.5
