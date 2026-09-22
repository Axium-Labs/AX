#!/usr/bin/env sh
# AX installer — one-line install from GitHub Releases.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Axium-Labs/AX/main/scripts/install.sh | sh
#
# Environment:
#   AX_VERSION       release tag to install (default: latest, e.g. v0.1.0)
#   AX_INSTALL_DIR   install directory (default: ~/.local/bin)
#
# The downloaded archive is verified against the release's SHA256SUMS before
# anything is written to disk.

set -eu

REPO="Axium-Labs/AX"
BASE_URL="https://github.com/${REPO}/releases"

# --- detect OS and architecture ---------------------------------------------
uname_s="$(uname -s)"
uname_m="$(uname -m)"

case "${uname_s}" in
    Linux) os="unknown-linux-musl" ;;
    Darwin) os="apple-darwin" ;;
    *)
        echo "AX: unsupported platform: ${uname_s}" >&2
        exit 1
        ;;
esac

case "${uname_m}" in
    x86_64 | amd64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *)
        echo "AX: unsupported architecture: ${uname_m}" >&2
        exit 1
        ;;
esac

# --- resolve version ---------------------------------------------------------
version="${AX_VERSION:-latest}"
if [ "${version}" = "latest" ]; then
    download_base="${BASE_URL}/latest/download"
else
    case "${version}" in
        v*) ;;
        *) version="v${version}" ;;
    esac
    download_base="${BASE_URL}/download/${version}"
fi

asset="ax-${arch}-${os}.tar.gz"
asset_url="${download_base}/${asset}"
sums_url="${download_base}/SHA256SUMS"

# --- install location ---------------------------------------------------------
install_dir="${AX_INSTALL_DIR:-${HOME}/.local/bin}"
mkdir -p "${install_dir}"

# --- download and verify ------------------------------------------------------
tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

echo "AX: downloading ${asset_url}"
curl -fsSL "${asset_url}" -o "${tmp_dir}/ax.tar.gz"
curl -fsSL "${sums_url}" -o "${tmp_dir}/SHA256SUMS"

expected="$(awk -v asset="${asset}" '$2 == asset { print $1 }' "${tmp_dir}/SHA256SUMS")"
if [ -z "${expected}" ]; then
    echo "AX: ${asset} is missing from SHA256SUMS" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${tmp_dir}/ax.tar.gz" | awk '{ print $1 }')"
else
    # macOS ships shasum instead of sha256sum.
    actual="$(shasum -a 256 "${tmp_dir}/ax.tar.gz" | awk '{ print $1 }')"
fi

if [ "${expected}" != "${actual}" ]; then
    echo "AX: checksum mismatch" >&2
    echo "  expected: ${expected}" >&2
    echo "  actual:   ${actual}" >&2
    exit 1
fi

# --- extract and install -------------------------------------------------------
tar -xzf "${tmp_dir}/ax.tar.gz" -C "${tmp_dir}"
install -m 0755 "${tmp_dir}/ax" "${install_dir}/ax"
echo "AX: installed to ${install_dir}/ax"

# --- PATH hint -----------------------------------------------------------------
case ":${PATH}:" in
    *":${install_dir}:"*) ;;
    *)
        echo "AX: ${install_dir} is not on your PATH."
        echo "    Add it with:"
        echo "      export PATH=\"${install_dir}:\$PATH\""
        echo "    and put that line in your shell profile (~/.bashrc, ~/.zshrc, ...)."
        ;;
esac

# --- verify ---------------------------------------------------------------------
"${install_dir}/ax" --version
