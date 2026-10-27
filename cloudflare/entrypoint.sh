#!/bin/sh
set -eu
umask 077

: "${MEIBO_ORIGIN:?MEIBO_ORIGIN is required}"
: "${MEIBO_REMOTE_CONFIG:?MEIBO_REMOTE_CONFIG is required}"

printf '%s\n' "$MEIBO_REMOTE_CONFIG" > /tmp/meibo-remote.json
exec meibo hosted-remote /tmp/meibo-remote.json serve \
  --origin "$MEIBO_ORIGIN" --bind 0.0.0.0:4317
