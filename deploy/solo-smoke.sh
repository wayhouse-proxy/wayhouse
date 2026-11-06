#!/bin/sh
# Solo bundle smoke test (deploy/compose/compose.solo.yml). Assumes the stack is up under the
# compose project named in SOLO_PROJECT and COMPOSE says how to talk to it; run deploy/smoke.sh
# first for the checks the demo already has.
#   COMPOSE="docker compose -p wayhouse-solo-smoke ..." SOLO_PROJECT=wayhouse-solo-smoke sh deploy/solo-smoke.sh
set -eu
ENV_FILE="${COMPOSE_ENV_FILE:-deploy/compose/.env}"
# shellcheck disable=SC1090
. "$ENV_FILE"
: "${COMPOSE:?}" "${SOLO_PROJECT:?}"

fail() { echo "SOLO FAIL: $*" >&2; exit 1; }
retry() { # <desc> <cmd...> up to 60 s
  desc=$1; shift
  i=0
  until "$@" >/dev/null 2>&1; do
    i=$((i + 1))
    [ "$i" -ge 60 ] && fail "$desc (gave up after 60 s)"
    sleep 1
  done
  echo "ok: $desc"
}
# The env file is the one init.sh wrote: owner-only, random tokens, never overwritten.
[ "$(stat -c %a "$ENV_FILE")" = 600 ] || fail "$ENV_FILE is not mode 0600"
case "$WAYHOUSE_CONTROLLER_TOKEN$WAYHOUSE_UI_PASSWORD" in *change-me*) fail "an example token is in use" ;; esac
if sh "$(dirname "$0")/solo/init.sh" "$ENV_FILE" >/dev/null 2>&1; then fail "init.sh overwrote an existing env file"; fi
echo "ok: init.sh wrote a 0600 env file with random tokens and refuses to overwrite it"
CTRL=http://127.0.0.1:9901
UI=http://127.0.0.1:9903
auth="Authorization: Bearer $WAYHOUSE_CONTROLLER_TOKEN"

# The plugin API is on.
retry "controller serves the plugin API" curl -fsS -H "$auth" "$CTRL/plugins"

# The keyring exists, is owner-only and belongs to the controller's user.
keys_vol="${SOLO_PROJECT}_solo-keys"
inspect_keys() {
  docker run --rm -v "$keys_vol":/keys:ro busybox:1.37.0 sh -c 'stat -c "%a %u" /keys/secret.keys; sha256sum /keys/secret.keys'
}
before=$(inspect_keys)
case "$before" in "600 65532"*) echo "ok: keyring is 0600 and owned by 65532" ;; *) fail "keyring mode/owner: $before" ;; esac

# Sealing works end to end: a stored secret is listed by name and its value is nowhere in the answer.
code=$(curl -s -o /dev/null -w '%{http_code}' -X PUT -H "$auth" -H 'content-type: application/json' \
  -d '{"value":"solo-smoke-value-1234"}' "$CTRL/secrets/solo-smoke")
[ "$code" = 200 ] || fail "PUT /secrets/solo-smoke gave HTTP $code"
list=$(curl -fsS -H "$auth" "$CTRL/secrets")
echo "$list" | grep -q '"solo-smoke"' || fail "the stored secret is not listed"
if echo "$list" | grep -q 'solo-smoke-value'; then fail "GET /secrets shows the value"; fi
echo "ok: a secret is sealed, listed by name, and never shown"

# Pushing a config through the UI's BFF works with a session cookie.
cookie=$(curl -sS -D - -o /dev/null -X POST -H 'content-type: application/json' \
  -d "{\"password\":\"$WAYHOUSE_UI_PASSWORD\"}" "$UI/ui/login" | sed -n 's/^[Ss]et-[Cc]ookie: \([^;]*\).*/\1/p' | head -n 1)
[ -n "$cookie" ] || fail "ui login gave no session cookie"
code=$(curl -s -o /dev/null -w '%{http_code}' -H "Cookie: $cookie" -X POST --data-binary @deploy/compose/wayhouse.yaml "$UI/api/config")
[ "$code" = 200 ] || fail "POST /api/config through the UI gave HTTP $code"
echo "ok: a config pushed through the UI is accepted"

# A second start of the key step must not change the key.
$COMPOSE run --rm keys >/dev/null
after=$(inspect_keys)
[ "$before" = "$after" ] || fail "the keyring changed on a second run"
echo "ok: the keyring survives a second start unchanged"

# The controller restarts cleanly and still holds the secret.
$COMPOSE restart controller >/dev/null
retry "controller is healthy after a restart" curl -fsS "$CTRL/healthz"
curl -fsS -H "$auth" "$CTRL/secrets" | grep -q '"solo-smoke"' || fail "the secret is gone after a restart"
echo "ok: the secret survives a restart"

# No token, password or secret value in any log.
logs=$($COMPOSE logs --no-color 2>&1)
for v in "$WAYHOUSE_CONTROLLER_TOKEN" "$WAYHOUSE_AGGREGATOR_TOKEN" "$WAYHOUSE_INGEST_TOKEN" "$WAYHOUSE_UI_PASSWORD" solo-smoke-value-1234; do
  if printf '%s' "$logs" | grep -qF -- "$v"; then fail "a secret value appears in the logs"; fi
done
echo "ok: no token, password or secret value in the logs"

echo "SOLO PASS"
