#!/usr/bin/env bash
# Ensure gitleaks is available (PATH or .tools/). Does not scan.
# Usage: ./scripts/bootstrap-gitleaks.sh [--quiet]

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
QUIET=0

for arg in "$@"; do
  case "$arg" in
    --quiet) QUIET=1 ;;
  esac
done

info() {
  if [[ "$QUIET" -eq 0 ]]; then
    echo "$*"
  fi
}

find_gitleaks() {
  if command -v gitleaks >/dev/null 2>&1; then
    command -v gitleaks
    return 0
  fi
  if [[ -x "$ROOT/.tools/gitleaks" ]]; then
    echo "$ROOT/.tools/gitleaks"
    return 0
  fi
  return 1
}

if path="$(find_gitleaks)"; then
  info "[+] gitleaks: $path"
  exit 0
fi

version="8.24.2"
os="$(uname -s)"
arch="$(uname -m)"
case "$os-$arch" in
  Linux-x86_64|Linux-amd64) asset="linux_x64" ;;
  Linux-aarch64|Linux-arm64) asset="linux_arm64" ;;
  Darwin-x86_64) asset="darwin_x64" ;;
  Darwin-arm64|Darwin-aarch64) asset="darwin_arm64" ;;
  *)
    echo "[!] Unsupported platform: $os $arch — install gitleaks manually." >&2
    exit 1
    ;;
esac

url="https://github.com/gitleaks/gitleaks/releases/download/v${version}/gitleaks_${version}_${asset}.tar.gz"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

info "[*] Downloading gitleaks (first use)..."
curl -fsSL "$url" -o "$tmpdir/gitleaks.tar.gz"
tar -xzf "$tmpdir/gitleaks.tar.gz" -C "$tmpdir"
mkdir -p "$ROOT/.tools"
install -m 755 "$tmpdir/gitleaks" "$ROOT/.tools/gitleaks"
info "[+] gitleaks installed: $ROOT/.tools/gitleaks"
