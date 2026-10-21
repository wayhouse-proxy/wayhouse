#!/bin/sh
# Exercises the failing path of check-caddy-modules.sh: two modules on one xcaddy line, one
# licensed and named in the NOTICE, one not. The check must fail and name only the second.
set -eu
cd "$(dirname "$0")/.."
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
printf 'RUN xcaddy build --with example.com/a/one --with example.com/b/two@v1\n' >"$t/Dockerfile"
printf 'example.com/a/one Apache-2.0\n' >"$t/MODULES"
printf 'example.com/a/one\n' >"$t/NOTICE"
if out=$(sh deploy/check-caddy-modules.sh "$t/Dockerfile" "$t/MODULES" "$t/NOTICE" 2>&1); then
  echo "selftest: check passed with an unlicensed module" >&2; exit 1
fi
case $out in *example.com/b/two*) ;; *) echo "selftest: second module not reported: $out" >&2; exit 1 ;; esac
case $out in *"example.com/a/one has"*|*"example.com/a/one is"*) echo "selftest: licensed module wrongly reported: $out" >&2; exit 1 ;; esac
echo "ok: check-caddy-modules catches an unlicensed module among several"
