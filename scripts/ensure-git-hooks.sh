#!/usr/bin/env bash
# Point this clone at tracked git hooks (core.hooksPath). Idempotent.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
HOOKS_PATH="scripts/git-hooks"
QUIET=0

for arg in "$@"; do
  case "$arg" in
    --quiet) QUIET=1 ;;
  esac
done

if [[ ! -d "$ROOT/.git" ]]; then
  [[ "$QUIET" -eq 1 ]] || echo "[!] Not a git repository — skipping hook setup." >&2
  exit 0
fi

if [[ ! -f "$ROOT/$HOOKS_PATH/pre-commit" ]]; then
  echo "[!] Missing $HOOKS_PATH/pre-commit" >&2
  exit 1
fi

current="$(git -C "$ROOT" config --get core.hooksPath 2>/dev/null || true)"
if [[ "$current" == "$HOOKS_PATH" ]]; then
  [[ "$QUIET" -eq 1 ]] || echo "[+] Git hooks already configured (core.hooksPath=$HOOKS_PATH)"
  exit 0
fi

git -C "$ROOT" config core.hooksPath "$HOOKS_PATH"
if [[ "$QUIET" -eq 1 ]]; then
  :
else
  echo "[+] Configured git hooks (core.hooksPath=$HOOKS_PATH)"
  echo "    Commits will scan staged secrets automatically."
fi
