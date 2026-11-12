#!/usr/bin/env bash
# Onyx CLI Universal Installer for Linux and macOS (x86_64, aarch64/ARM64, ARM32)
set -euo pipefail

REPO="EMP0RI0M/onyx-cli"
INSTALL_DIR="${ONYX_INSTALL_DIR:-/usr/local/bin}"

# Detect OS
OS="$(uname -s)"
case "${OS}" in
    Linux*)     PLATFORM="linux";;
    Darwin*)    PLATFORM="darwin";;
    *)          echo "Error: Unsupported OS '${OS}'."; exit 1;;
esac

# Detect Architecture
ARCH="$(uname -m)"
case "${ARCH}" in
    x86_64|amd64)   ARCH="amd64";;
    aarch64|arm64)  ARCH="arm64";;
    armv7*|armhf)   ARCH="arm32";;
    *)              echo "Error: Unsupported architecture '${ARCH}'."; exit 1;;
esac

ASSET="onyx-${PLATFORM}-${ARCH}"
echo "→ Detected target: ${PLATFORM} (${ARCH})"

# Fetch latest release tag
LATEST_TAG=$(curl -sSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/' || true)
if [ -z "${LATEST_TAG}" ]; then
    LATEST_TAG="v0.1.0"
fi

URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET}.tar.gz"
echo "→ Downloading Onyx CLI (${LATEST_TAG}) from:"
echo "  ${URL}"

TMP_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_DIR}"' EXIT

if curl -sSL -f "${URL}" -o "${TMP_DIR}/${ASSET}.tar.gz"; then
    tar -xzf "${TMP_DIR}/${ASSET}.tar.gz" -C "${TMP_DIR}"
    DOWNLOADED_BIN="${TMP_DIR}/${ASSET}"
else
    # Fallback if tarball not yet published: fetch binary directly
    BIN_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET}"
    curl -sSL -f "${BIN_URL}" -o "${TMP_DIR}/onyx"
    DOWNLOADED_BIN="${TMP_DIR}/onyx"
fi

chmod +x "${DOWNLOADED_BIN}"

# Installation target check
if [ ! -w "${INSTALL_DIR}" ]; then
    echo "→ Writing to ${INSTALL_DIR} requires sudo privileges:"
    sudo install -m 755 "${DOWNLOADED_BIN}" "${INSTALL_DIR}/onyx"
else
    install -m 755 "${DOWNLOADED_BIN}" "${INSTALL_DIR}/onyx"
fi

echo ""
echo "✓ Onyx CLI installed successfully to ${INSTALL_DIR}/onyx"
echo "Run 'onyx --help' or 'onyx' to start."
