#!/bin/sh
# check-caddy-image.sh <image>: the wayhouse-caddy image reports the Caddy version pinned in
# deploy/Dockerfile, starts with every capability dropped (a file capability on the binary
# would make that exec fail) and carries the base config the sync expects.
set -eu
img=$1
cd "$(dirname "$0")/.."
want=$(sed -n 's/^FROM caddy:\([0-9][0-9.]*\)@sha256:[0-9a-f]* AS wayhouse-caddy$/\1/p' deploy/Dockerfile)
[ -n "$want" ] || { echo "check-caddy-image: no pinned 'FROM caddy:<ver>@sha256:… AS wayhouse-caddy' in deploy/Dockerfile" >&2; exit 1; }
# The build stage compiles CADDY_VERSION; the final stage is the base image of the same tag.
# A bump of one without the other would ship a binary the base image does not match (#418).
built=$(sed -n 's/^ARG CADDY_VERSION=\([0-9][0-9.]*\)$/\1/p' deploy/Dockerfile)
[ "$built" = "$want" ] || { echo "check-caddy-image: Dockerfile builds Caddy '$built' but its base image is $want" >&2; exit 1; }
got=$(docker run --rm --cap-drop ALL --user 65532:65532 "$img" version)
case "$got" in
  v$want\ *|v$want) ;;
  *) echo "check-caddy-image: $img reports '$got', deploy/Dockerfile pins $want" >&2; exit 1 ;;
esac
# The binary itself must carry the pinned Go toolchain and the replaced modules (#418): a dropped
# --replace or a silently older toolchain would otherwise pass the version check above.
go_want=$(sed -n 's/^FROM --platform=\$BUILDPLATFORM golang:\([0-9][0-9.]*\)-alpine@sha256:[0-9a-f]* AS caddy-build$/\1/p' deploy/Dockerfile)
net_want=$(sed -n 's/^ARG GOLANG_X_NET_VERSION=\([0-9][0-9.]*\)$/\1/p' deploy/Dockerfile)
jose_want=$(sed -n 's/^ARG GO_JOSE_VERSION=\([0-9][0-9.]*\)$/\1/p' deploy/Dockerfile)
[ -n "$go_want" ] && [ -n "$net_want" ] && [ -n "$jose_want" ] || { echo "check-caddy-image: cannot read the Go, x/net and go-jose pins from deploy/Dockerfile" >&2; exit 1; }
info=$(docker run --rm --cap-drop ALL --user 65532:65532 "$img" build-info)
echo "$info" | grep -q "go$go_want" || { echo "check-caddy-image: the binary was not built with Go $go_want; build-info says:" >&2; echo "$info" >&2; exit 1; }
echo "$info" | grep -q "golang.org/x/net.* v$net_want" || { echo "check-caddy-image: the binary does not carry golang.org/x/net v$net_want; build-info says:" >&2; echo "$info" >&2; exit 1; }
echo "$info" | grep -q "go-jose/v4.* v$jose_want" || { echo "check-caddy-image: the binary does not carry go-jose/v4 v$jose_want; build-info says:" >&2; echo "$info" >&2; exit 1; }
# Our --replace pins must not go below what Caddy's own go.mod asks for, or a bump of Caddy would
# silently downgrade a dependency. A failed download is a warning, a confirmed downgrade fails.
mod=$(curl -fsS --max-time 20 "https://raw.githubusercontent.com/caddyserver/caddy/v$want/go.mod" 2>/dev/null || true)
check_not_below() {
  have=$(printf '%s\n' "$mod" | sed -n "s|^[[:space:]]*$1 v\([0-9][0-9.]*\).*|\1|p" | head -n 1)
  [ -n "$have" ] || { echo "check-caddy-image: could not read Caddy's $1 requirement (skipped)" >&2; return 0; }
  [ "$(printf '%s\n%s\n' "$have" "$2" | sort -V | head -n 1)" = "$have" ] ||
    { echo "check-caddy-image: Caddy $want requires $1 v$have, newer than the pinned v$2: bump the pin in deploy/Dockerfile" >&2; exit 1; }
}
check_not_below golang.org/x/net "$net_want"
check_not_below github.com/go-jose/go-jose/v4 "$jose_want"
docker run --rm --entrypoint cat "$img" /etc/caddy/base.json | grep -q '"persist": false' ||
  { echo "check-caddy-image: base.json lacks admin.config.persist=false" >&2; exit 1; }
echo "ok: $img is Caddy $want"
