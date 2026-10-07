#!/usr/bin/env bash
# The four clips of the README, recorded frame by frame and encoded by
# encode.sh.
#
#   scripts/media/clips.sh            all four
#   scripts/media/clips.sh hero       one of: hero, resume, panes, themes
set -u
MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
FRAMES="$MEDIA_ROOT/frames"

clip_hero() {
    bash "$ROOT/scripts/media/launch.sh" --reseed >/dev/null
    # shellcheck source=/dev/null
    . "$ROOT/scripts/media/shots.sh"
    goto "atlas"
    pause 1.0
    park_cursor
    start_recording "$FRAMES/hero"
    pause 0.5
    goto "login-timeout"
    pause 1.0
    combo ctrl shift a     # new agent session
    pause 1.4
    k Return               # Claude Code
    pause 4.2              # the shell starts, the agent draws its frame
    nudge 4.6              # let it work through the tool lines
    back_to_sidebar
    goto "rate-limits"     # look at another worktree: the session keeps running
    pause 2.0
    back_to_sidebar        # and come back to it from the tree
    k Up; pause 0.5
    k Up; pause 0.7
    k Return               # into the running session, where it was
    pause 2.2
    nudge 1.4
    pause 1.0
    stop_recording "$FRAMES/hero"
}

clip_resume() {
    bash "$ROOT/scripts/media/launch.sh" --reseed >/dev/null
    # shellcheck source=/dev/null
    . "$ROOT/scripts/media/shots.sh"
    goto "atlas"
    pause 1.0
    park_cursor
    start_recording "$FRAMES/resume"
    pause 0.4
    goto "close idle"      # Enter resumes it in a terminal
    pause 4.6
    combo ctrl shift l     # the stored transcript
    pause 3.2
    k Return               # back to the resumed terminal
    pause 1.8
    stop_recording "$FRAMES/resume"
}

clip_panes() {
    bash "$ROOT/scripts/media/launch.sh" --reseed >/dev/null
    # shellcheck source=/dev/null
    . "$ROOT/scripts/media/shots.sh"
    goto "login-timeout"
    pause 1.0
    park_cursor
    start_recording "$FRAMES/panes"
    pause 0.4
    open_shell             # a shell in the worktree
    pause 0.4
    text "git worktree list"; k Return
    pause 1.8
    combo ctrl shift d     # split the pane to the right
    pause 2.0
    text "git log --oneline -3"; k Return
    pause 1.8
    combo ctrl shift bracketright
    pause 1.4
    combo ctrl shift bracketleft
    pause 1.2
    stop_recording "$FRAMES/panes"
}

clip_themes() {
    bash "$ROOT/scripts/media/launch.sh" --reseed >/dev/null
    # shellcheck source=/dev/null
    . "$ROOT/scripts/media/shots.sh"
    goto "atlas"
    pause 1.0
    park_cursor
    start_recording "$FRAMES/themes"
    pause 0.4
    combo ctrl shift y     # light
    pause 1.6
    combo ctrl shift y     # dark
    pause 1.4
    combo ctrl shift j     # next theme: Zavu
    pause 2.0
    combo ctrl shift h     # back to Leon
    pause 1.4
    stop_recording "$FRAMES/themes"
}

case "${1:-all}" in
    hero) clip_hero ;;
    resume) clip_resume ;;
    panes) clip_panes ;;
    themes) clip_themes ;;
    all) clip_hero; clip_resume; clip_panes; clip_themes ;;
    *) echo "usage: clips.sh [hero|resume|panes|themes|all]" >&2; exit 2 ;;
esac
