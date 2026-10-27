#!/bin/sh
# Install the pinned Linux x86_64 fixture; the optional archive enables verified caching.
set -eu

if [ "$#" -lt 1 ] || [ "$#" -gt 2 ]; then
    printf '%s\n' 'usage: install-ci-sqld.sh INSTALL_DIRECTORY [ARCHIVE]' >&2
    exit 2
fi
destination=$1
if [ -e "$destination/sqld" ] || [ -L "$destination/sqld" ]; then
    printf '%s\n' 'sqld destination already exists' >&2
    exit 1
fi

stage=$(mktemp -d "${TMPDIR:-/tmp}/meibo-ci-sqld.XXXXXXXX")
trap 'rm -rf "$stage"' EXIT HUP INT TERM
archive=${2:-"$stage/sqld.tar.xz"}
if [ "$#" -eq 1 ]; then
    curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
        --output "$archive" \
        https://github.com/tursodatabase/libsql/releases/download/libsql-server-v0.24.32/libsql-server-x86_64-unknown-linux-gnu.tar.xz
fi
if command -v sha256sum >/dev/null 2>&1; then
    checksum=$(sha256sum "$archive")
elif command -v shasum >/dev/null 2>&1; then
    checksum=$(shasum -a 256 "$archive")
else
    printf '%s\n' 'a SHA-256 checker is required' >&2
    exit 1
fi
checksum=${checksum%% *}
if [ "$checksum" != '71720fc8648c19efef416efebd47145ef59b62e198770533530a858e1336879f' ]; then
    printf '%s\n' 'sqld archive checksum mismatch' >&2
    exit 1
fi
tar -xJf "$archive" -C "$stage" --strip-components=1 \
    libsql-server-x86_64-unknown-linux-gnu/sqld
test -f "$stage/sqld"
test ! -L "$stage/sqld"
mkdir -p "$destination"
(set -C; cat "$stage/sqld" > "$destination/sqld")
chmod 0755 "$destination/sqld"
printf '%s\n' "$destination/sqld"
