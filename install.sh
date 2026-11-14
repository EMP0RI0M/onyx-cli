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
mkdir -p "${INSTALL_DIR}" 2>/dev/null || true

# If running as root or user has ~/.local/bin in PATH, update all matching onyx binaries found
USER_BIN="${HOME}/.local/bin"
mkdir -p "${USER_BIN}" 2>/dev/null || true

if [ -w "${INSTALL_DIR}" ]; then
    install -m 755 "${DOWNLOADED_BIN}" "${INSTALL_DIR}/onyx"
elif command -v sudo >/dev/null 2>&1; then
    echo "→ Writing to ${INSTALL_DIR} requires sudo privileges:"
    sudo install -m 755 "${DOWNLOADED_BIN}" "${INSTALL_DIR}/onyx"
else
    # Fallback to user home directory if root/sudo is unavailable
    FALLBACK_DIR="${HOME}/.local/bin"
    mkdir -p "${FALLBACK_DIR}"
    install -m 755 "${DOWNLOADED_BIN}" "${FALLBACK_DIR}/onyx"
    INSTALL_DIR="${FALLBACK_DIR}"
    echo "→ Installed to ${FALLBACK_DIR}/onyx (make sure ${FALLBACK_DIR} is in your PATH)."
fi

# Also update ~/.local/bin/onyx if it exists or if INSTALL_DIR was different
if [ -d "${USER_BIN}" ] && [ -w "${USER_BIN}" ] && [ "${INSTALL_DIR}" != "${USER_BIN}" ]; then
    if [ -f "${USER_BIN}/onyx" ]; then
        install -m 755 "${DOWNLOADED_BIN}" "${USER_BIN}/onyx"
    fi
fi

# ---------------------------------------------------------------------------
# CLIProxyAPI Integration: Ensure gateway binary is available for OAuth/proxy
# ---------------------------------------------------------------------------
CLIPROXY_BIN_DIR="${HOME}/.cli-proxy-api"
CLIPROXY_BIN="${CLIPROXY_BIN_DIR}/cli-proxy-api"

if ! command -v cli-proxy-api >/dev/null 2>&1 && ! command -v cliproxyapi-rust >/dev/null 2>&1 && [ ! -f "${CLIPROXY_BIN}" ]; then
    echo "→ Checking CLIProxyAPI gateway..."
    mkdir -p "${CLIPROXY_BIN_DIR}" 2>/dev/null || true

    # Map architecture for CLIProxyAPI releases
    CP_ARCH="${ARCH}"
    case "${ARCH}" in
        amd64) CP_ARCH="amd64";;
        arm64) CP_ARCH="arm64";;
        arm32) CP_ARCH="armv7";;
        *)     CP_ARCH="${ARCH}";;
    esac

    CP_REPO="router-for-me/CLIProxyAPI"
    CP_TAG=$(curl -sSL "https://api.github.com/repos/${CP_REPO}/releases/latest" 2>/dev/null | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/' || true)
    if [ -n "${CP_TAG}" ]; then
        CP_URL="https://github.com/${CP_REPO}/releases/download/${CP_TAG}/CLIProxyAPI_${CP_TAG#v}_${PLATFORM}_${CP_ARCH}.tar.gz"
        echo "→ Setting up CLIProxyAPI background proxy gateway (${CP_TAG})..."
        if curl -sSL -f "${CP_URL}" -o "${TMP_DIR}/cliproxy.tar.gz" 2>/dev/null; then
            tar -xzf "${TMP_DIR}/cliproxy.tar.gz" -C "${TMP_DIR}" 2>/dev/null || true
            for cand in "${TMP_DIR}/cli-proxy-api" "${TMP_DIR}/cliproxyapi-rust" "${TMP_DIR}/CLIProxyAPI"*; do
                if [ -f "${cand}" ] && [ -x "${cand}" ]; then
                    install -m 755 "${cand}" "${CLIPROXY_BIN}" 2>/dev/null || cp "${cand}" "${CLIPROXY_BIN}"
                    chmod +x "${CLIPROXY_BIN}"
                    # Symlink into INSTALL_DIR if writable
                    if [ -w "${INSTALL_DIR}" ]; then
                        ln -sf "${CLIPROXY_BIN}" "${INSTALL_DIR}/cliproxyapi-rust" 2>/dev/null || true
                    fi
                    echo "✓ CLIProxyAPI gateway installed to ${CLIPROXY_BIN}"
                    break
                fi
            done
        fi
    fi
fi

# Ensure default ~/.onyx configuration directory exists
mkdir -p "${HOME}/.onyx" 2>/dev/null || true

echo ""
echo "✓ Onyx CLI installed successfully to ${INSTALL_DIR}/onyx"
echo "Run 'onyx --help' or 'onyx' to start."
