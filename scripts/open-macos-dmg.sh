#!/usr/bin/env bash
# Install and open AKA.app from an unsigned/ad-hoc macOS DMG.
#
# Usage:
#   scripts/open-macos-dmg.sh dist/aka-desktop-<ver>-aarch64-apple-darwin.dmg
#   scripts/open-macos-dmg.sh --system ~/Downloads/aka-desktop-<ver>-aarch64-apple-darwin.dmg
set -euo pipefail

INSTALL_DIR="${AKA_MACOS_INSTALL_DIR:-${HOME}/Applications}"
OPEN_AFTER=1

usage() {
  sed -n '4,8p' "${BASH_SOURCE[0]}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --system)
      INSTALL_DIR="/Applications"
      shift
      ;;
    --install-dir)
      [[ $# -ge 2 ]] || { echo "error: --install-dir needs a path" >&2; exit 1; }
      INSTALL_DIR="$2"
      shift 2
      ;;
    --no-open)
      OPEN_AFTER=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      break
      ;;
    -*)
      echo "error: unknown option $1" >&2
      usage >&2
      exit 1
      ;;
    *)
      break
      ;;
  esac
done

DMG="${1:-}"
if [[ -z "${DMG}" ]]; then
  DMG="$(find "${PWD}/dist" -maxdepth 1 -type f -name 'aka-desktop-*-apple-darwin.dmg' 2>/dev/null | sort | tail -n 1 || true)"
fi
[[ -n "${DMG}" && -f "${DMG}" ]] || { echo "error: DMG not found" >&2; usage >&2; exit 1; }

mkdir -p "${INSTALL_DIR}"
MOUNT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/aka-dmg.XXXXXX")"

cleanup() {
  hdiutil detach "${MOUNT_DIR}" -quiet >/dev/null 2>&1 || true
  rmdir "${MOUNT_DIR}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "==> removing quarantine from ${DMG}"
xattr -dr com.apple.quarantine "${DMG}" 2>/dev/null || true

echo "==> verifying ${DMG}"
hdiutil verify "${DMG}" >/dev/null

echo "==> mounting ${DMG}"
hdiutil attach "${DMG}" -mountpoint "${MOUNT_DIR}" -nobrowse -readonly >/dev/null

SRC_APP="${MOUNT_DIR}/AKA.app"
DEST_APP="${INSTALL_DIR}/AKA.app"
[[ -d "${SRC_APP}" ]] || { echo "error: AKA.app not found in DMG" >&2; exit 1; }

echo "==> installing ${DEST_APP}"
rm -rf "${DEST_APP}"
ditto --noqtn "${SRC_APP}" "${DEST_APP}"

echo "==> removing Gatekeeper quarantine"
xattr -dr com.apple.quarantine "${DEST_APP}" 2>/dev/null || true

echo "==> verifying app signature"
codesign --verify --deep --strict --verbose=2 "${DEST_APP}"

if [[ "${OPEN_AFTER}" -eq 1 ]]; then
  echo "==> opening AKA"
  open "${DEST_APP}"
fi

echo "==> done: ${DEST_APP}"
