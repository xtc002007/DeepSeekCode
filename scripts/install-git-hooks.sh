#!/usr/bin/env bash
# Optional manual setup (same as auto `cargo build` hook configuration).
# Usage:
#   ./scripts/install-git-hooks.sh
#   ./scripts/install-git-hooks.sh --install-gitleaks

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
INSTALL_GITLEAKS=0

for arg in "$@"; do
  case "$arg" in
    --install-gitleaks) INSTALL_GITLEAKS=1 ;;
    -h|--help)
      echo "Usage: $0 [--install-gitleaks]"
      exit 0
      ;;
    *)
      echo "Unknown option: $arg" >&2
      exit 1
      ;;
  esac
done

"$ROOT/scripts/ensure-git-hooks.sh"

if [[ "$INSTALL_GITLEAKS" -eq 1 ]]; then
  "$ROOT/scripts/bootstrap-gitleaks.sh" || true
fi

echo ""
echo "Normal git workflow works: git add -> git commit -> git push"
echo "Full-repo scan (release): ./scripts/release/scan-secrets.ps1"
