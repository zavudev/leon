#!/usr/bin/env bash
# Builds the demo workspace inside the sandbox, so the git worktree metadata
# is spelled the way Leon will see it (/home/ada/...).
set -euo pipefail
MEDIA_ROOT="${MEDIA_ROOT:-/tmp/leon-media}"
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$MEDIA_ROOT/home"

# The repo lives under the real /home, which the sandbox replaces with a
# tmpfs, so it is bound again under /tmp where the sandbox can see it.
bwrap \
    --dev-bind / / \
    --tmpfs /home \
    --bind "$MEDIA_ROOT/home" /home/ada \
    --ro-bind "$ROOT" /tmp/leon-media-src \
    --setenv HOME /home/ada \
    bash /tmp/leon-media-src/scripts/media/setup-demo.sh /home/ada
