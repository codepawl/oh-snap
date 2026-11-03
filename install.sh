#!/bin/sh
# Installs oh-snap: downloads the latest release for this OS and CPU, checks it against
# SHA256SUMS.txt, then lets `oh-snap install` set up start at sign-in and the shortcuts.
#   curl -fsSL https://raw.githubusercontent.com/codepawl/oh-snap/main/install.sh | sh
set -eu

release_url="https://github.com/codepawl/oh-snap/releases/latest/download"

fail() {
    echo "oh-snap install: $*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

# Prints the release asset name for this machine.
asset_name() {
    system=$(uname -s)
    machine=$(uname -m)
    case "$system" in
        Linux)
            case "$machine" in
                x86_64 | amd64) echo "oh-snap-linux-x86_64.tar.gz" ;;
                aarch64 | arm64) echo "oh-snap-linux-aarch64.tar.gz" ;;
                *) fail "no build for Linux on $machine" ;;
            esac
            ;;
        *) fail "no build for $system yet" ;;
    esac
}

# Prints the SHA-256 of a file.
file_hash() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        fail "sha256sum or shasum is required to check the download"
    fi
}

need curl
need tar

asset=$(asset_name)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

echo "downloading $asset"
curl -fsSL -o "$work/$asset" "$release_url/$asset" || fail "download failed: $release_url/$asset"
curl -fsSL -o "$work/SHA256SUMS.txt" "$release_url/SHA256SUMS.txt" || fail "download failed: SHA256SUMS.txt"

expected=$(grep " \*\{0,1\}$asset\$" "$work/SHA256SUMS.txt" | cut -d ' ' -f 1 | head -n 1)
[ -n "$expected" ] || fail "SHA256SUMS.txt has no entry for $asset"
actual=$(file_hash "$work/$asset")
[ "$expected" = "$actual" ] || fail "checksum mismatch for $asset (expected $expected, got $actual)"

tar -xzf "$work/$asset" -C "$work" oh-snap || fail "could not unpack $asset"
chmod +x "$work/oh-snap"
"$work/oh-snap" install
