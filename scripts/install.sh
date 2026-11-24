#!/usr/bin/env sh
# One-line installer for GLASTOR zerho (Linux x86_64 & macOS ARM64/x86_64)
# Usage: curl -fsSL https://raw.githubusercontent.com/glastor-core/jester/main/scripts/install.sh | sh
set -eu

REPO="glastor-core/jester"
INSTALL_DIR="${ZERHO_INSTALL_DIR:-$HOME/.local/bin}"

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)
    case "$ARCH" in
      x86_64|amd64) ARCHIVE="zerho-linux-x86_64.tar.gz" ;;
      *) echo "error: unsupported Linux architecture: $ARCH" >&2; exit 1 ;;
    esac
    ;;
  Darwin)
    case "$ARCH" in
      arm64|aarch64) ARCHIVE="zerho-macos-arm64.tar.gz" ;;
      x86_64) ARCHIVE="zerho-macos-x86_64.tar.gz" ;;
      *) echo "error: unsupported macOS architecture: $ARCH" >&2; exit 1 ;;
    esac
    ;;
  *)
    echo "error: unsupported OS: $OS (use scripts/install.ps1 on Windows)" >&2
    exit 1
    ;;
esac

BASE_URL="https://github.com/${REPO}/releases/latest/download"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "Downloading ${ARCHIVE} from ${REPO}..."
curl -fsSL "${BASE_URL}/${ARCHIVE}" -o "${TMP_DIR}/${ARCHIVE}"
curl -fsSL "${BASE_URL}/${ARCHIVE}.sha256" -o "${TMP_DIR}/${ARCHIVE}.sha256"

echo "Verifying SHA-256 checksum..."
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$TMP_DIR" && sha256sum -c "${ARCHIVE}.sha256")
elif command -v shasum >/dev/null 2>&1; then
  (cd "$TMP_DIR" && shasum -a 256 -c "${ARCHIVE}.sha256")
else
  echo "warning: sha256sum/shasum not found; skipping checksum verification" >&2
fi

mkdir -p "$INSTALL_DIR"
tar -xzf "${TMP_DIR}/${ARCHIVE}" -C "$INSTALL_DIR"
chmod +x "${INSTALL_DIR}/zerho"

echo "Installed zerho to ${INSTALL_DIR}/zerho"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "Add to PATH: export PATH=\"${INSTALL_DIR}:\$PATH\"" ;;
esac
"${INSTALL_DIR}/zerho" --version || true
