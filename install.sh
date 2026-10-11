#!/bin/sh
# Install a prebuilt MemCtrl binary from GitHub Releases, verified against SHA256SUMS.
#
#   curl -fsSL https://raw.githubusercontent.com/KJ-AIML/memctrl/main/install.sh | sh
#
# Environment:
#   MEMCTRL_VERSION      release tag to install, e.g. v2.0.0 (default: latest)
#   MEMCTRL_INSTALL_DIR  destination directory (default: $HOME/.local/bin)
#   MEMCTRL_BASE_URL     download base override, for mirrors and tests
set -eu

repo="KJ-AIML/memctrl"
version="${MEMCTRL_VERSION:-latest}"
dest="${MEMCTRL_INSTALL_DIR:-$HOME/.local/bin}"

fail() {
    echo "memctrl: $*" >&2
    exit 1
}

case "$(uname -s)/$(uname -m)" in
    Linux/x86_64 | Linux/amd64) target=x86_64-unknown-linux-musl ;;
    Linux/aarch64 | Linux/arm64) target=aarch64-unknown-linux-musl ;;
    Darwin/arm64) target=aarch64-apple-darwin ;;
    Darwin/x86_64) target=x86_64-apple-darwin ;;
    *) fail "no prebuilt binary for $(uname -s)/$(uname -m); build from source: cargo install --locked memctrl" ;;
esac

if [ -n "${MEMCTRL_BASE_URL:-}" ]; then
    base="$MEMCTRL_BASE_URL"
elif [ "$version" = latest ]; then
    base="https://github.com/$repo/releases/latest/download"
else
    base="https://github.com/$repo/releases/download/$version"
fi

command -v curl >/dev/null 2>&1 || fail "curl is required"
archive="memctrl-$target.tar.gz"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "memctrl: downloading $archive ($version)"
curl -fsSL "$base/$archive" -o "$tmp/$archive" || fail "download failed: $base/$archive"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || fail "download failed: $base/SHA256SUMS"

expected=$(awk -v file="$archive" '$2 == file { print $1 }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || fail "$archive is not listed in SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$archive" | awk '{ print $1 }')
else
    actual=$(shasum -a 256 "$tmp/$archive" | awk '{ print $1 }')
fi
[ "$expected" = "$actual" ] || fail "checksum mismatch for $archive; refusing to install"

tar -xzf "$tmp/$archive" -C "$tmp"
mkdir -p "$dest"
install -m 0755 "$tmp/memctrl-$target/memctrl" "$dest/memctrl"
echo "memctrl: installed $("$dest/memctrl" --version) to $dest/memctrl"
case ":$PATH:" in
    *":$dest:"*) ;;
    *) echo "memctrl: add $dest to your PATH" ;;
esac
