#!/bin/sh
# check-caddy-modules.sh: every Caddy module built into wayhouse-caddy (`--with <module>` in
# deploy/Dockerfile) has a recognised license recorded in deploy/caddy/MODULES and is named in
# deploy/caddy/NOTICE. No daemon needed.
set -eu
cd "$(dirname "$0")/.."
known="Apache-2.0 MIT BSD-2-Clause BSD-3-Clause ISC MPL-2.0"
rc=0
for mod in $(grep -v '^[[:space:]]*#' deploy/Dockerfile | sed -n 's/.*--with[ =]\([^ @=\\]*\).*/\1/p'); do
  lic=$(sed -n "s|^$mod[[:space:]]\{1,\}||p" deploy/caddy/MODULES | head -n 1)
  case " $known " in
    *" $lic "*) [ -n "$lic" ] || { echo "check-caddy-modules: $mod has no license in deploy/caddy/MODULES" >&2; rc=1; } ;;
    *) echo "check-caddy-modules: $mod has unrecognised license '$lic' (known: $known)" >&2; rc=1 ;;
  esac
  grep -q "$mod" deploy/caddy/NOTICE || { echo "check-caddy-modules: $mod is not named in deploy/caddy/NOTICE" >&2; rc=1; }
done
[ "$rc" = 0 ] && echo "ok: no unlicensed Caddy module in deploy/Dockerfile"
exit "$rc"
