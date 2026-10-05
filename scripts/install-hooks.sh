#!/bin/sh
# Installs a git pre-push hook that runs scripts/check.sh. Opt-in: nothing runs
# this for you. Remove the hook by deleting .git/hooks/pre-push (or, with
# worktrees, the file `git rev-parse --git-path hooks/pre-push` names).
#
#   scripts/install-hooks.sh
#
# Skip it once with `git push --no-verify`; CI will still run the same checks.

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
hook=$(git -C "$root" rev-parse --git-path hooks/pre-push)
case "$hook" in
    /*) ;;
    *) hook="$root/$hook" ;;
esac

if [ -e "$hook" ] && ! grep -q 'scripts/check.sh' "$hook"; then
    echo "install-hooks: $hook already exists and is not ours; not touching it." >&2
    exit 1
fi

mkdir -p "$(dirname "$hook")"
cat >"$hook" <<'EOF'
#!/bin/sh
# Installed by scripts/install-hooks.sh: run the local gate before every push.
root=$(git rev-parse --show-toplevel)
exec "$root/scripts/check.sh"
EOF
chmod +x "$hook"
echo "Installed $hook (runs scripts/check.sh before every push)."
