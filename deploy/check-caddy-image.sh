#!/bin/sh
# check-caddy-image.sh <image>: the wayhouse-caddy image reports the Caddy version pinned in
# deploy/Dockerfile, starts with every capability dropped (a file capability on the binary
# would make that exec fail) and carries the base config the sync expects.
set -eu
img=$1
cd "$(dirname "$0")/.."
want=$(sed -n 's/^FROM caddy:\([0-9][0-9.]*\)@sha256:[0-9a-f]* AS wayhouse-caddy$/\1/p' deploy/Dockerfile)
[ -n "$want" ] || { echo "check-caddy-image: no pinned 'FROM caddy:<ver>@sha256:… AS wayhouse-caddy' in deploy/Dockerfile" >&2; exit 1; }
got=$(docker run --rm --cap-drop ALL --user 65532:65532 "$img" version)
case "$got" in
  v$want\ *|v$want) ;;
  *) echo "check-caddy-image: $img reports '$got', deploy/Dockerfile pins $want" >&2; exit 1 ;;
esac
docker run --rm --entrypoint cat "$img" /etc/caddy/base.json | grep -q '"persist": false' ||
  { echo "check-caddy-image: base.json lacks admin.config.persist=false" >&2; exit 1; }
echo "ok: $img is Caddy $want"
