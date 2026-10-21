#!/bin/sh
# check-caddy-modules.sh [dockerfile modules notice]: every Caddy module built into
# wayhouse-caddy (`--with <module>` in deploy/Dockerfile) has a recognised license recorded in
# deploy/caddy/MODULES and is named in deploy/caddy/NOTICE. No daemon needed. The optional
# arguments let the self-test (check-caddy-modules-selftest.sh) point it at fixtures.
set -eu
cd "$(dirname "$0")/.."
df=${1:-deploy/Dockerfile}
modules=${2:-deploy/caddy/MODULES}
notice=${3:-deploy/caddy/NOTICE}
known="Apache-2.0 MIT BSD-2-Clause BSD-3-Clause ISC MPL-2.0"
rc=0
# grep -o yields every `--with` on a line (several per `xcaddy build` line is the usual form).
for mod in $(grep -v '^[[:space:]]*#' "$df" | grep -o -- '--with[ =][^ @=\\]*' | sed 's/--with[ =]//'); do
  lic=$(awk -v m="$mod" '$1 == m { print $2; exit }' "$modules")
  case " $known " in
    *" $lic "*) [ -n "$lic" ] || { echo "check-caddy-modules: $mod has no license in $modules" >&2; rc=1; } ;;
    *) echo "check-caddy-modules: $mod has unrecognised license '$lic' (known: $known)" >&2; rc=1 ;;
  esac
  grep -qF -- "$mod" "$notice" || { echo "check-caddy-modules: $mod is not named in $notice" >&2; rc=1; }
done
[ "$rc" = 0 ] && echo "ok: no unlicensed Caddy module in $df"
exit "$rc"
