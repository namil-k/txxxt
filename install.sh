#!/bin/sh
set -e

REPO="kimnam1/txxxt"
INSTALL_DIR="/usr/local/bin"
BINARY="txxxt"

# Detect OS and architecture
OS=$(uname -s)
ARCH=$(uname -m)

case "${OS}" in
  Darwin)
    case "${ARCH}" in
      arm64) ASSET="txxxt-macos-arm64.tar.gz" ;;
      x86_64) ASSET="txxxt-macos-x86_64.tar.gz" ;;
      *) echo "Unsupported architecture: ${ARCH}"; exit 1 ;;
    esac
    ;;
  Linux)
    case "${ARCH}" in
      x86_64) ASSET="txxxt-linux-x86_64.tar.gz" ;;
      *) echo "Unsupported architecture: ${ARCH}"; exit 1 ;;
    esac
    ;;
  *)
    echo "Unsupported OS: ${OS} (try downloading from GitHub releases)"
    exit 1
    ;;
esac

# Get latest release download URL
DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${ASSET}"

echo "Installing txxxt..."
echo "  OS: ${OS} ${ARCH}"
echo "  From: ${DOWNLOAD_URL}"

# Download and extract to temp dir
TMP_DIR=$(mktemp -d)
trap 'rm -rf "${TMP_DIR}"' EXIT

curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_DIR}/${ASSET}"
tar xzf "${TMP_DIR}/${ASSET}" -C "${TMP_DIR}"

# Ensure install directory exists
if [ ! -d "${INSTALL_DIR}" ]; then
  echo "  Creating ${INSTALL_DIR}"
  sudo mkdir -p "${INSTALL_DIR}"
fi

# Install
if [ -w "${INSTALL_DIR}" ]; then
  mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
else
  echo "  Need sudo to install to ${INSTALL_DIR}"
  sudo mv "${TMP_DIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
fi

chmod +x "${INSTALL_DIR}/${BINARY}"

echo ""
echo "Done! Run: txxxt"
