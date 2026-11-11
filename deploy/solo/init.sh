#!/bin/sh
# Writes the tokens and the UI password a solo box needs into an env file (default
# deploy/compose/.env), random and readable by you only. It never prints a value and never
# overwrites a file that exists, so running it twice cannot change a token under a running stack.
#   sh deploy/solo/init.sh [FILE]
# With --credentials DIR it writes one file per secret instead (controller-token, aggregator-token,
# ingest-token, ui-password): the sources for systemd's LoadCredential= in deploy/systemd/.
#   sh deploy/solo/init.sh --credentials /etc/wayhouse/credentials
set -eu
umask 077
rand() { od -An -N"$1" -tx1 /dev/urandom | tr -d ' \n'; }
if [ "${1:-}" = "--credentials" ]; then
  dir="${2:?usage: init.sh --credentials DIR}"
  if [ -e "$dir/controller-token" ]; then
    echo "$dir/controller-token exists; not touching it. Delete the files first to start over." >&2
    exit 1
  fi
  mkdir -p "$dir"
  chmod 0700 "$dir"
  for name in controller-token aggregator-token ingest-token; do rand 24 > "$dir/$name"; done
  rand 12 > "$dir/ui-password"
  chmod 0600 "$dir"/*
  echo "wrote $dir/{controller-token,aggregator-token,ingest-token,ui-password} (mode 0600). The values stay out of your terminal."
  exit 0
fi
file="${1:-$(dirname "$0")/../compose/.env}"
if [ -e "$file" ]; then
  echo "$file exists; not touching it. Delete it first to start over." >&2
  exit 1
fi
{
  echo "WAYHOUSE_CONTROLLER_TOKEN=$(rand 24)"
  echo "WAYHOUSE_AGGREGATOR_TOKEN=$(rand 24)"
  echo "WAYHOUSE_INGEST_TOKEN=$(rand 24)"
  echo "WAYHOUSE_UI_PASSWORD=$(rand 12)"
} > "$file"
chmod 0600 "$file"
echo "wrote $file (mode 0600). The UI password is WAYHOUSE_UI_PASSWORD in it; the tokens stay out of your terminal and your shell history."
