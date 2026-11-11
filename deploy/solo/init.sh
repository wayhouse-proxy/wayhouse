#!/bin/sh
# Writes the tokens and the UI password a solo box needs into an env file (default
# deploy/compose/.env), random and readable by you only. It never prints a value and never
# overwrites a file that exists, so running it twice cannot change a token under a running stack.
#   sh deploy/solo/init.sh [FILE]
set -eu
umask 077
file="${1:-$(dirname "$0")/../compose/.env}"
if [ -e "$file" ]; then
  echo "$file exists; not touching it. Delete it first to start over." >&2
  exit 1
fi
rand() { od -An -N"$1" -tx1 /dev/urandom | tr -d ' \n'; }
{
  echo "WAYHOUSE_CONTROLLER_TOKEN=$(rand 24)"
  echo "WAYHOUSE_PROXY_TOKEN=$(rand 24)"
  echo "WAYHOUSE_AGENT_TOKEN=$(rand 24)"
  echo "WAYHOUSE_AGGREGATOR_TOKEN=$(rand 24)"
  echo "WAYHOUSE_INGEST_TOKEN=$(rand 24)"
  echo "WAYHOUSE_UI_PASSWORD=$(rand 12)"
} > "$file"
chmod 0600 "$file"
echo "wrote $file (mode 0600). The UI password is WAYHOUSE_UI_PASSWORD in it; the tokens stay out of your terminal and your shell history."
