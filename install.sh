#!/bin/sh
# Installs brim from the GitHub releases.
#
#   curl -fsSL https://brimlang.com/install.sh | sh
#
# Options (as environment variables):
#   BRIM_VERSION   a version such as 0.1.1 (default: the latest release)
#   BRIM_INSTALL   the directory for brim (default: /usr/local/bin if it is
#                  writable, else ~/.local/bin)
#   BRIM_NATIVE    0 for the build without the LLVM backend (no `brim build`)
#   BRIM_NO_ICONS  1 to skip the file icons for Finder and Linux desktops
#                  (`brim icons`)
set -eu

repo="ordnary-com/brim"

fail() {
    echo "error: $*" >&2
    exit 1
}

need() {
    command -v "$1" > /dev/null 2>&1 || fail "this script needs \`$1\`"
}

need curl
need tar
need uname

os=$(uname -s)
arch=$(uname -m)
case "$os/$arch" in
    Darwin/arm64) platform="macos-aarch64"; native=1 ;;
    Linux/x86_64 | Linux/amd64) platform="linux-x86_64"; native=1 ;;
    Linux/aarch64 | Linux/arm64) platform="linux-aarch64"; native=0 ;;
    Darwin/x86_64) fail "there is no build for Intel Macs yet; build from source: https://github.com/$repo" ;;
    *) fail "there is no build for $os on $arch; build from source: https://github.com/$repo" ;;
esac
if [ "${BRIM_NATIVE:-1}" = "0" ]; then
    native=0
fi
if [ "$native" = "1" ]; then
    platform="$platform-native"
fi

version="${BRIM_VERSION:-}"
if [ -z "$version" ]; then
    # The newest release, pre-releases included (0.x versions are pre-releases).
    version=$(curl -fsSL "https://api.github.com/repos/$repo/releases?per_page=1" \
        | sed -n 's/.*"tag_name": *"v\([^"]*\)".*/\1/p' | head -n 1)
    [ -n "$version" ] || fail "cannot find the latest release of $repo"
fi
version="${version#v}"

archive="brim-$version-$platform.tar.gz"
base="https://github.com/$repo/releases/download/v$version"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "downloading brim $version ($platform)"
curl -fsSL "$base/$archive" -o "$tmp/$archive" || fail "cannot download $base/$archive"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || fail "cannot download the checksums"

expected=$(grep " $archive\$" "$tmp/SHA256SUMS" | cut -d ' ' -f 1)
[ -n "$expected" ] || fail "$archive is not listed in SHA256SUMS"
if command -v sha256sum > /dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$archive" | cut -d ' ' -f 1)
else
    actual=$(shasum -a 256 "$tmp/$archive" | cut -d ' ' -f 1)
fi
[ "$expected" = "$actual" ] || fail "the checksum of $archive does not match"

tar xzf "$tmp/$archive" -C "$tmp"

dir="${BRIM_INSTALL:-}"
if [ -z "$dir" ]; then
    if [ -w /usr/local/bin ]; then
        dir=/usr/local/bin
    else
        dir="$HOME/.local/bin"
    fi
fi
mkdir -p "$dir"
cp "$tmp/brim-$version-$platform/brim" "$dir/brim"
chmod 755 "$dir/brim"

echo "installed $("$dir/brim" --version) in $dir"
# File icons for .brim and .brimproj files, except on servers and in CI.
if [ "${BRIM_NO_ICONS:-0}" != "1" ] && [ -z "${CI:-}" ] \
    && { [ "$os" = "Darwin" ] || [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; }; then
    "$dir/brim" icons || true
fi
case ":$PATH:" in
    *":$dir:"*) ;;
    *) echo "note: $dir is not on your PATH; add it, for example in ~/.zshrc or ~/.bashrc:"
       echo "  export PATH=\"$dir:\$PATH\"" ;;
esac
if [ "$native" = "1" ] && ! command -v cc > /dev/null 2>&1; then
    echo "note: \`brim build\` needs a C compiler (cc) to link executables"
fi
