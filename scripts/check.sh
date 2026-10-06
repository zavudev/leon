#!/bin/sh
# The local gate: what CI runs for this computer's platform, in the same order,
# plus a few cheap guards for the platforms this computer is not. Stops at the
# first failure and prints a summary with the time of every step.
#
#   scripts/check.sh            the whole gate
#   scripts/check.sh --quick    only the fast steps (format, hygiene, generated docs)
#   cargo xtask check           the same
#
# Run it before every push: CI is a confirmation, not the first test. Install
# it as a pre-push hook with scripts/install-hooks.sh (opt-in).

set -u

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root" || exit 1

quick=0
for arg in "$@"; do
    case "$arg" in
        --quick) quick=1 ;;
        -h | --help)
            sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "check.sh: unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

# A build directory of CI's kind: no incremental state (it is large and CI
# never reuses it). Callers that want it back can export CARGO_INCREMENTAL=1.
CARGO_INCREMENTAL=${CARGO_INCREMENTAL:-0}
export CARGO_INCREMENTAL
CARGO_TERM_COLOR=${CARGO_TERM_COLOR:-always}
export CARGO_TERM_COLOR

summary=""
notes=""
overall_start=$(date +%s)

record() { # record <PASS|FAIL|SKIP> <name> <seconds>
    summary="$summary
$(printf '  %-5s %-52s %4ss' "$1" "$2" "$3")"
}

finish() {
    total=$(($(date +%s) - overall_start))
    echo
    echo "==== check summary ===="
    printf '%s\n' "$summary"
    if [ -n "$notes" ]; then
        echo
        printf 'Notes:%s\n' "$notes"
    fi
    echo
    if [ "$1" -eq 0 ]; then
        echo "PASS  all steps passed in ${total}s"
    else
        echo "FAIL  stopped at the first failing step after ${total}s"
    fi
    exit "$1"
}

step() { # step <name> <command...>
    name=$1
    shift
    echo
    echo "==> $name"
    echo "    \$ $*"
    start=$(date +%s)
    "$@"
    status=$?
    seconds=$(($(date +%s) - start))
    if [ "$status" -ne 0 ]; then
        record FAIL "$name" "$seconds"
        finish 1
    fi
    record PASS "$name" "$seconds"
}

skip() { # skip <name> <why>
    record SKIP "$1" 0
    notes="$notes
  - skipped $1: $2"
}

# ----- hygiene (no build) -------------------------------------------------

# No tracked text file may carry a carriage return: the generated documents are
# compared byte for byte, and a script with CRLF does not run. Vendored licence
# texts under the fonts are the only exception.
no_crlf() {
    cr=$(printf '\r')
    found=$(git grep --cached -lI "$cr" -- . ':!crates/app/assets/fonts/*')
    if [ -n "$found" ]; then
        echo "files with CRLF line endings:" >&2
        printf '%s\n' "$found" >&2
        return 1
    fi
}

step "format (cargo fmt --all --check)" cargo fmt --all --check
step "no CRLF in tracked text files" no_crlf
step "Cargo.lock is current (cargo metadata --locked)" \
    sh -c 'cargo metadata --locked --format-version 1 >/dev/null'

if [ "$quick" -eq 1 ]; then
    step "generated docs are current" \
        cargo test -p leon --locked --quiet -- the_settings_reference_is_current \
        the_token_reference_in_the_docs
    finish 0
fi

# ----- what CI runs -------------------------------------------------------

step "clippy (workspace, all targets, -D warnings)" \
    cargo clippy --workspace --all-targets --locked -- -D warnings
step "tests (workspace, --no-fail-fast)" \
    cargo test --workspace --locked --no-fail-fast

# ----- guards for the platforms this computer is not ---------------------

# The shortcut registry holds the macOS and the other chord table side by side
# and its tests walk both (`for mac in [true, false]`), so one run on any
# computer checks the chords every platform will see.
step "key registry, both chord tables" cargo test -p leon --locked --quiet keys::

# The window and shell suites again, as the platforms this computer is not.
# `LEON_SIMULATE_OS` (read by crates/app/src/platform.rs in test builds only)
# makes the chord table, the key routing (a bare Ctrl chord belongs to the
# program in a terminal off macOS) and the "this computer has no POSIX shell"
# decisions follow the named platform, so a test that presses the macOS chord
# in a terminal, or expects a local reading on Windows, fails here and not
# first in CI. It decides logic only: real files, processes, clipboards and
# windows stay this computer's, so a test that depends on those is named below
# with its reason, never skipped silently.
host_os=$(uname -s)
sim_tests() { # sim_tests <linux|windows> <cargo test arguments after --skip...>
    sim=$1
    shift
    step "tests as $sim (LEON_SIMULATE_OS=$sim, leon app)" \
        env LEON_SIMULATE_OS="$sim" \
        cargo test -p leon --locked --no-fail-fast -- "$@"
}
case "$host_os" in
    Darwin)
        # Skipped when simulating on a Mac, each an artefact of the simulation:
        #  - down_moves_into_the_filtered_tree_...: it types Ctrl+A into a text
        #    field; the toolkit binds select-all in text fields for the real
        #    platform (Cmd+A here), so Ctrl+A cannot select all on a Mac.
        sim_tests linux --skip down_moves_into_the_filtered_tree_and_enter_opens_the_best_match
        sim_tests windows --skip down_moves_into_the_filtered_tree_and_enter_opens_the_best_match
        notes="$notes
  - the simulated runs skip 1 test (down_moves_into_the_filtered_tree_...: the toolkit's text-field select-all is bound for the real platform)"
        ;;
    Linux)
        sim_tests windows
        ;;
    *)
        skip "tests as another platform" "no simulation is set up for $host_os"
        ;;
esac
notes="$notes
  - the simulated runs cover decisions (chords, key routing, \"no POSIX shell\"), not the real OS: file systems, processes, clipboard, windowing and path syntax of Windows or Linux are only seen by CI"

# Path identity as Windows spells it: git's `C:/x`, an agent's `C:\x`, the
# verbatim prefix, case. A session folder must match its worktree whatever the
# spelling, in the store, in the tree and when the store is healed.
step "path spellings, as Windows (leon-core, history roots, the tree)" \
    sh -c 'cargo test -p leon-core --locked --quiet -- path:: spelling migration \
        && cargo test -p leon-history --locked --quiet -- roots:: cut_mid_line zero_length \
        && LEON_SIMULATE_OS=windows cargo test -p leon --locked --quiet -- backslash_folder same_folder'

# Generated documents (settings reference, token reference) match their source.
step "generated docs are current" \
    cargo test -p leon --locked --quiet -- the_settings_reference_is_current \
    the_token_reference_in_the_docs

# A type check for the other desktop targets. Only the crates without a C
# dependency can be checked without that target's C toolchain (SQLite and ring
# need one), so the rest is named as skipped, never failed.
pure_crates="leon-wire leon-pty"
c_crates="leon-core leon-history leon-remote leon-usage leon-link leon-host leon-term leon-mark leon (C dependencies or GPUI)"
installed=$(rustup target list --installed 2>/dev/null || true)
host=$(rustc -vV | sed -n 's/^host: //p')
for target in x86_64-unknown-linux-gnu x86_64-pc-windows-msvc; do
    if [ "$target" = "$host" ]; then
        continue
    fi
    if ! printf '%s\n' "$installed" | grep -qx "$target"; then
        skip "cross check $target" "target not installed (rustup target add $target)"
        continue
    fi
    args=""
    for crate in $pure_crates; do
        args="$args -p $crate"
    done
    # shellcheck disable=SC2086
    # Warnings are errors, as in CI's clippy: code that only `cfg(windows)`
    # (or only unix) leaves unused fails there, so it fails here.
    step "type check for $target ($pure_crates)" \
        env RUSTFLAGS="-D warnings" \
        cargo check --locked --all-targets --target "$target" $args
    notes="$notes
  - $target: not type-checked here, they need that target's C toolchain: $c_crates"
done

finish 0
