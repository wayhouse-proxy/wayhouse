#!/bin/sh
# Thin wrapper over `wayhouse-controller init`, which generates the tokens, the UI password and a
# private CA with the controller's and aggregator's TLS certificates. It never prints a secret
# value and never overwrites a file that exists, so running it twice cannot change a token under a
# running stack. Same arguments as before:
#   sh deploy/solo/init.sh [FILE]
#     compose: writes FILE (default deploy/compose/.env) and a tls/ directory next to it.
#   sh deploy/solo/init.sh --credentials DIR
#     systemd: one file per secret (controller-token, proxy-token, agent-token, aggregator-token,
#     ingest-token, ui-password) in DIR, the sources for LoadCredential= in deploy/systemd/, and the
#     certificates in DIR/../tls.
# The binary is `wayhouse-controller` on PATH, or WAYHOUSE_CONTROLLER_BIN (a path, or a command such
# as "cargo run -q -p wayhouse-controller --"). The program has other targets (k8s-secret) and
# options (--san for the names other hosts reach the controller by): see `wayhouse-controller init --help`.
set -eu
bin="${WAYHOUSE_CONTROLLER_BIN:-wayhouse-controller}"
if ! command -v "${bin%% *}" >/dev/null 2>&1; then
  echo "init.sh needs wayhouse-controller: put it on PATH or set WAYHOUSE_CONTROLLER_BIN" >&2
  exit 127
fi
# $bin may be a command with arguments, so it is deliberately not quoted.
if [ "${1:-}" = "--credentials" ]; then
  dir="${2:?usage: init.sh --credentials DIR}"
  # shellcheck disable=SC2086
  exec $bin init --target systemd --dir "$(dirname "$dir")" --credentials-dir "$dir"
fi
file="${1:-$(dirname "$0")/../compose/.env}"
# shellcheck disable=SC2086
exec $bin init --target compose --dir "$(dirname "$file")" --env-file "$file"
