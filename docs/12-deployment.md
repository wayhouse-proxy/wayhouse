# 12 – Container images & deployment

How to package the five binaries for Docker/Kubernetes, and — the part that
isn't obvious going in — how a proxy that needs to listen on many, changing
ports actually works once it's namespaced by a container runtime.

## Container images

The reference build is [`deploy/Dockerfile`](../deploy/Dockerfile): one file, a
shared `rust:1-trixie` builder and seven runtime targets (the five binaries, `wayhouse-minimal`, below, and
`wayhouse-caddy`, [web proxy](web-proxy.md)) on
`gcr.io/distroless/cc-debian13:nonroot` (Caddy on its own pinned base; see [`deploy/README.md`](../deploy/README.md);
`make deploy-images` builds all seven; `make deploy-scan` runs an informational Trivy
scan of them and of the lockfiles they're built from — CI's `deploy` and `trivy` jobs).
`wayhouse-minimal` is `wayhouse` built with `--no-default-features` (issue #62): no WASM sniffer loader,
gRPC resolver, `dns_srv` source, Tier-2 gossip fabric or WireGuard tunnel client, so the
binary is much smaller. It takes the same arguments as `wayhouse`; a config or flag that needs a
dropped feature (`settings.sniffers`, a `grpc` resolver, a `dns_srv` source, `settings.gossip`,
a `tunnel` source, `--tunnel-iface`) is refused at startup and under `--check`, naming the
cargo feature.

The published `ghcr.io` images are public and need no login to pull.

None of these five binaries shell out to an
external command at runtime (WireGuard interface management in `wayhouse`/`wayhouse-agent`
goes through kernel netlink directly via `defguard/wireguard-rs`, not the
`ip`/`wg` CLIs) — so the runtime image needs nothing but the binary and its
dynamic library dependencies (glibc). It does not need a system CA bundle either: the
HTTP clients (`--controller`, `--aggregator`, `--tunnel-controller-url`, the HTTP
resolvers) verify against a root bundle compiled into the binary, plus whatever
`--ca-file` adds — see
[TLS for the fleet services](#tls-for-the-fleet-services) for what that implies. The builder and runtime base
must be on the same Debian release, or a binary can fail to start on an older glibc.

`[profile.release]` sets `strip = true`, which trims roughly 15-20% off every binary.

Measured image sizes (`docker images`, stripped release binaries, `deploy/Dockerfile`
on `distroless/cc-debian13:nonroot`, CI run 36922142697, 2026-10-01):

| Binary | image size |
|---|---|
| `wayhouse` | **47.4 MB** |
| `wayhouse-controller` | **37.9 MB** |
| `wayhouse-agent` | 34.3 MB |
| `wayhouse-ui` | 34.3 MB (includes the built frontend) |
| `wayhouse-aggregator` | 33.3 MB |

For comparison, an earlier 2026-09-07 measurement on `cc-debian12` gave 45-65 MB.

The base image dominates, not the binary — `debian:trixie-slim` alone is
~118 MB before anything is added; `distroless/cc-debian13` (glibc +
libgcc/libstdc++, no shell, no package manager) is a fraction of that and
was confirmed to actually run these binaries (no runtime shell-out, so
distroless's missing shell is never a problem). Trade-off: no
`docker exec ... sh` for interactive debugging, and the `nonroot` variant
runs as a fixed non-root uid (fine here — nothing in this project needs
root once its sockets are bound; see `--tunnel-*`'s capability
requirements below for the one exception).

`wayhouse` and `wayhouse-controller` are the two heaviest either way, for real
reasons: `wayhouse` links `wasmtime` (phase 9 sniffers) and
`defguard_wireguard_rs`/`boringtun` (phase 14 backend transport);
`wayhouse-controller` links `openraft` + `redb` for the HA/Raft log store
(phase 12). `wayhouse-agent`/`wayhouse-aggregator`/`wayhouse-ui` don't carry those.

## Examples

[`deploy/compose/`](../deploy/compose/) is a runnable control-plane demo (controller,
aggregator, UI, one `wayhouse` pulling its config from the controller) with a tunnel
override; [`deploy/k8s/`](../deploy/k8s/) has Kustomize manifests (`kubectl apply -k deploy/k8s/overlays/single`, plus `ha` and `web-gateway` overlays) (the proxy as a
`hostNetwork` DaemonSet, plus the RBAC its `kubernetes` sources need). Both are reference only (the images themselves are published to GHCR on version tags by `.github/workflows/release.yml`). CI smoke-tests the compose demo and schema-validates the manifests (the `deploy`
job, blocking since 2026-10-02; first green run 2026-10-01). Aggregator intent fan-out (drain etc.) goes to the `admin_url`
each `wayhouse` reports, by default `http(s)://<settings.admin.listen>`; when that
address is not reachable from the aggregator (`0.0.0.0`, loopback, a container
port mapping, a TLS terminator in front), set `wayhouse --aggregator-admin-url`. The
k8s DaemonSet reports `http://<node IP>:9900`, and the aggregator presents
`--instance-token` (the admin `auth_token`). The compose demo cannot use
fan-out: its `wayhouse` admin API listens on the host's loopback only, which the
bridge-networked aggregator cannot reach.

The aggregator separates the credentials: `--ingest-token` is what the `wayhouse`
instances push with (`--aggregator-token` on `wayhouse`; it unlocks only
`POST /ingest`), `--auth-token` is for `wayhouse-ui` and operators (`/fleet/*`). Setting
`--auth-token` requires a different `--ingest-token`; the aggregator refuses to
start otherwise.
It only stores an `admin_url` whose host is the pushing connection's own source
IP (never a loopback address), or one covered by `--instance-url-allow` (CIDR, hostname or `*.suffix`,
repeatable); set the allowlist (`127.0.0.0/8` on a single box) when the aggregator sees a different source
address than the one reported (NAT, a load balancer, a TLS terminator, the
compose bridge, or a child tier relaying through `--parent-url`). A refused push
gets a `400` and a warning in the aggregator's log. A hostname entry trusts
whoever controls DNS for that name, so prefer CIDRs where the addresses are stable. See `docs/10`, "The
aggregator".

## One box (the solo bundle)

Every feature works on a single instance: nothing here needs a second node. The solo bundle is the
compose demo plus what a single box needs for plugins and sealed secrets, all on the host's
loopback:

```sh
cd deploy/compose
wayhouse-controller init --target compose   # once: tokens in .env (mode 0600), CA and certificates in tls/; prints no secret
docker compose -f docker-compose.yml -f compose.solo.yml up -d --build
```

`sh ../solo/init.sh` is the same command for a checkout without the binary on `PATH` (it runs
`$WAYHOUSE_CONTROLLER_BIN`). Open <http://127.0.0.1:9903>; the password is `WAYHOUSE_UI_PASSWORD` in `.env`. Compose passes the tokens to each container
as a Docker secret file under `/run/secrets`, never on a command line. `compose.solo.yml`
adds a one-shot `keys` step that creates the sealed-secret keyring on first start and never
replaces it, and starts the controller with `--plugins` and `--secret-key-file`, so plugin secrets
and `{ secret: name }` config references work (UI: Settings, Secrets). **Back up the `solo-keys`
volume with the controller's data**: without the key every stored secret is unreadable.

**https from the first start.** `init` makes a private CA and two certificates under `tls/`: the
controller's (valid for `localhost`, `127.0.0.1`, `::1` and the service name `controller`) and the
aggregator's. The `keys` step copies them into the `solo-tls` volume (keys `0600` for uid 65532,
certificates public), the controller and aggregator serve TLS from them, and the UI and the proxy
trust only that CA (`--ca-file=/tls/ca.pem`). So neither plain-http guard has to be switched off:
the overlay sets no `--allow-insecure-controller` and no `--allow-insecure-secrets`. The published
ports are loopback only. The UI itself stays on plain http on the host loopback; put a TLS front
door (`compose.web.yml`) in front of it to expose it. To reach the controller from another host,
add its name with `init --san controller.example.org` (before the first start; `init` never
overwrites). The certificates are valid for five years and the CA for ten; there is no renewal
command yet, so replace the files in `tls/` (the controller re-reads them without a restart on
systemd, and the `keys` step copies them again on the next `docker compose up`). Add
`compose.web.yml` for the Caddy front door and `compose.solo-acme.yml` for certificates
([`acme.md`](acme.md#one-box)).

Without containers, [`deploy/systemd/`](../deploy/systemd/) has the same bundle as units
(`sudo wayhouse-controller init --target systemd` writes `/etc/wayhouse/credentials` and
`/etc/wayhouse/tls`, then `systemctl enable --now wayhouse-solo.target`). The controller serves
https on `127.0.0.1:9901` from `/etc/wayhouse/tls/controller.pem` (its key arrives as a systemd
credential) and the other units trust `/etc/wayhouse/tls/ca.pem`; the aggregator stays on loopback
http, which the guard allows. There is no supervisor binary: systemd or Docker's restart policy
does that job. Tokens reach each program as files (`LoadCredential=` or Docker secrets), never as
flags. CI runs `make deploy-solo-smoke` (the certificates and key modes, https-only controller,
keyring mode and owner, a sealed secret that is listed but never shown, a config pushed through the
UI, a clean restart, no token in any log).

### All in one process (`wayhouse-control`)

A solo box can run the controller, the aggregator and the UI as **one process**, `wayhouse-control`,
instead of three. It links the three programs as they are (no logic moved), so every route, token and
flag behaves as in the split layout. The split binaries and images stay the way to run a fleet:
aggregators are stateless replicas, controllers are Raft nodes, and neither scales inside one process.

```sh
# compose (from deploy/compose)
wayhouse-controller init --target compose --control      # one certificate, tls/control.pem
docker compose -f docker-compose.yml -f compose.solo.yml -f compose.solo-control.yml up -d --build

# systemd
sudo wayhouse-controller init --target systemd --control # control.pem/control.key under /etc/wayhouse/tls
sudo systemctl enable --now wayhouse-solo-control.target
```

What stays the same: three listeners (controller `9901`, aggregator `9902`, UI `9903`), three tokens
and the UI password, the same keyring, data volume and settings volume, and a proxy that enrolls or is
configured against `https://127.0.0.1:9901` unchanged. No enroll role was added: the control plane is
single-node, so nothing joins it as a replica.

What differs:

- **One certificate.** `control.pem` is valid for `localhost`, `127.0.0.1`, `::1` and the names the
  one process answers to (`controller` in compose, plus any `--san`). The UI dials the other two
  over loopback with their own tokens, so the code path equals the split layout.
- **The UI serves https in compose.** It listens on the container's `0.0.0.0`, and `wayhouse-control`
  refuses a non-loopback UI without its own certificate (a login over plain http would send the
  password in the clear). Open <https://127.0.0.1:9903> and accept the browser warning. Do not add
  `tls/ca.pem` to a browser or system trust store: the init CA has no name constraints, so it could
  vouch for any name. Keep `ca.key` out of backups. On systemd the UI stays on loopback http.
- **Flags are namespaced.** `--controller-X`, `--aggregator-X` and `--ui-X` reach program `X` (a UI
  flag that already starts with `ui-` keeps it: `--ui-ui-password-file`). `--settings-dir`,
  `--tls-cert`, `--tls-key` and `--ca-file` are shared; the UI's own TLS is `--ui-tls-cert` and
  `--ui-tls-key`. A flag a program derives from the others (`--ui-controller-url`, the UI's tokens,
  every `--listen`) is refused or pinned, so a settings file cannot move a listener the UI depends
  on. `wayhouse-control --help` lists the rules. Each program keeps its own settings file
  (`controller.json`, `aggregator.json`, `ui.json`).
- **Secrets are per program.** The three programs name their secrets alike (`auth-token`), so under
  systemd each credential is `<program>-<flag>` (`controller-auth-token`, `aggregator-ingest-token`,
  `ui-ui-password`; see `deploy/systemd/wayhouse-control.service`), and compose gives each its own
  `--<program>-…-file`. `wayhouse-control` clears `CREDENTIALS_DIRECTORY` before it starts and refuses
  `WAYHOUSE_*` secret variables.
- **The token split is nominal inside the process.** Each listener still checks its own token, but one
  compromised process holds the controller's state (revision logs, ACME keys, sealed secrets) and the
  fleet instance token together. That is acceptable with one operator on one box, and it is why a
  fleet keeps the split binaries. The units keep the systemd hardening of the split ones.
- **Crash coupling.** Release builds abort on a panic, so one component cannot restart alone. If any
  of the three stops, the process exits non-zero and systemd (or `restart: unless-stopped`) restarts
  all three.
- **`/metrics` lists every component.** The three share one recorder, so each listener's `/metrics`
  carries the series of all three.

**Moving on to the split layout or to HA** (nothing is lost; the data directory, keyring, tokens and
settings are the same files). Stop the process, then copy `control.pem` and `control.key` to
`controller.*` and `aggregator.*` in the same `tls/` directory (the CA stays), switch the units or
the compose overlay back (leave out `compose.solo-control.yml`; on systemd use
`wayhouse-solo.target`), and start. On systemd that is all: the split units reach everything on
loopback. In compose the split UI dials `https://aggregator:9902`, a name `control.pem` does not
carry, and there is no command yet to issue a leaf (the same gap as certificate renewal), so run
`init --san aggregator` into a new `tls/` directory before the first start if you expect to split
later. To run Raft replicas, continue with
[Upgrading a single controller to HA](#upgrading-a-single-controller-to-ha); that is a controller-binary
feature, and `wayhouse-control` runs one node only. Kubernetes keeps separate Deployments, as before.

CI runs `make deploy-solo-control-smoke` against the compose overlay: the same checks as the split
solo smoke, against one container. The systemd unit is checked with `systemd-analyze verify` and a
unit test that feeds its `ExecStart` and credential names through the plan; a real `systemctl start`
is on the pre-release manual list.

`wayhouse-controller init --target k8s-secret | kubectl apply -f -` prints three Secret manifests
(`wayhouse-tokens` with every key the example Secret has, `wayhouse-keyring`, `wayhouse-tls` with
the CA certificate and the two leaves, not the CA key) for the `wayhouse` namespace (`--namespace`).
It refuses to print to a terminal (use `--output FILE`). The Kubernetes manifests mount that `wayhouse-tls`
Secret (create it before `kubectl apply -k`): the controller and the aggregator serve https from it, the UI and
the node proxy trust its `ca.crt` (`--ca-file`), the probes use `scheme: HTTPS`, and the HA overlay's replicas
dial each other at `https://<pod>.wayhouse-controller-peers:9901` (the leaf names `*.wayhouse-controller-peers`).
Only the aggregator keeps `--allow-insecure-controller`: it presents the instance token to each node's admin API,
which the DaemonSet exposes as `http://<node IP>:9900`, and no certificate covers a node IP. The certificates
name the `wayhouse` namespace; pass `--namespace` and `--san` to `init` for another. `deploy/lint.sh` fails the
build if any other pod brings the flag back or reaches a Service over plain http.
`ca.key` in `tls/` can sign a certificate for any name; once you have the certificates you need, move it offline (it is never mounted into a container, and `tls/` is git-ignored). Every `init` run also prints the controller certificate's SHA-256 fingerprint, which is public:
`wayhouse-agent enroll --pin` takes it, or pass the CA with `--controller-ca`.

## Process flags from a settings file (`--settings-dir`)

The proxy, the controller, the aggregator and the UI accept `--settings-dir DIR`. The program
reads `DIR/<program>.json` (`wayhouse`, `controller`, `aggregator`, `ui`) and uses its values as
flags the command line did not give: **the command line always wins**, and the programs read no
environment variables for their flags. The UI writes these files (Settings → Process flags);
the flag list it offers and the check of a new value come from each program's own `clap`
definition, so a flag added to a program shows up there with no second list.

A change is a **trial**. The file keeps the previous values and a `pending` mark, the program
restarts through its restart policy, and:

- if nobody confirms within 60 s of the start, the program puts the previous values back and
  restarts again;
- if the new values stop the program from starting, the next start goes back after three tries or
  five minutes, whichever comes first.

This needs a restart policy that restarts on exit status 75: systemd `Restart=on-failure` (the
units in `deploy/systemd`) or a compose `restart: unless-stopped`. There is no supervisor process.
The directory must be writable by the program and by the UI backend, and both must run as the **same
user** (the file is owner-only and every writer becomes its owner), so on one box it is one shared
volume; across hosts each program needs its own directory and the UI cannot reach it (use the
command line there). The shipped systemd units use `ProtectSystem=strict`: give each unit
`ReadWritePaths=` (or a `StateDirectory=`) for the directory. A program that finds a pending
change in a directory it cannot write does not try it: it starts with the previous values and does
not watch the file.

Not settable here: token, password and PSK flags, the flags that say where tokens go or what is
trusted (`*-url`, `*-file`, `--controller`, `--aggregator`, `--config`, `--static-dir`, `--ha-peers`, `--ha-join`, `--ha-import-source`, `--instance-url-allow`, `--acme-ca`, `--acme-challenge-upstream`, TLS certificate and key paths) and the
safety overrides (`--insecure-*`, `--allow-insecure-*`): set them on the command line or in the unit,
`--settings-dir` itself, and flags that run once and exit (`--check`, `--hash-password`,
`--secret-set`, `--secret-delete`, `--secret-list`). A key the running build does not know is
ignored with a warning, so a file written by a newer build never stops an older one. `--check`,
`--help` and the run-once modes do not use up a try. A file that is not valid JSON (a number
written without quotes, for example) is ignored whole with one warning; a confirmed file that
later stops parsing has no automatic undo, so remove `--settings-dir` or the file.

The UI's **Process flags** page (Settings, admin to change) edits these files: it lists each
program's flags, applies a change as a trial and shows Confirm and Undo while it waits. It needs
the UI itself to run with the same `--settings-dir` and as the same user. The solo compose bundle
sets this up (one `solo-settings` volume shared by all four programs); on systemd add
`--settings-dir=/var/lib/wayhouse/settings` to each unit, give them one `User=`, and list the
directory in `ReadWritePaths=` or `StateDirectory=`. The settings directory is a trust boundary:
whoever can write it can change the other programs' ordinary flags, so keep it to the programs
of one fleet node.

## Networking: a proxy that binds many, changing ports

`wayhouse` is designed to add and remove listeners live — `ListenerManager`
(`crates/wayhouse-core/src/listeners.rs`) reconciles the running listener set
from config on every SIGHUP/reload, and `bind: "0.0.0.0:30000-30999"`
(`docs/01` F1.4, `docs/08` roadmap) spawns one real socket per port in a
range under a single listener config, for fleets that hand out one port
per match/instance. None of that is Docker- or Kubernetes-aware — it's
just `bind()`/`listen()` calls inside the container's network namespace.
**And that's exactly the problem**: neither platform lets a running
container dynamically "claim" a new externally-reachable port just by
calling `bind()`. Port exposure is a static declaration in both:

- **Docker**: `-p hostport:containerport` sets up a NAT/iptables rule at
  container start. `EXPOSE`/`-P` is documentation, not a live mechanism.
  There's no API for a running container to add another published port —
  the container has to be recreated with a new `-p` list.
- **Kubernetes**: a `Service`'s `spec.ports` is a fixed list, and a
  container's own `ports:` field in the Pod spec is documentation only
  (Kubernetes doesn't firewall ports you didn't declare, and it doesn't
  publish ones you didn't declare either). Nothing watches a container's
  live sockets and reconciles a `Service` to match them.

Two patterns actually work, and `wayhouse` already has one foot in each:

1. **Host networking** (`docker run --network host` /
   `hostNetwork: true` on the Pod) — the container shares the host's
   network namespace directly, so every `bind()` `wayhouse` does is immediately
   live with zero orchestration involvement. This is the closest match to
   `wayhouse`'s own reload-driven listener reconciliation: add a `bind:` entry,
   send SIGHUP (or let file-watch pick it up), and the new listener exists
   — in a container exactly like bare metal. Cost: no network-namespace
   isolation for that pod/container — an acceptable, often expected,
   trade-off for the thing that sits at the actual network edge.
2. **A pre-reserved port range** — publish/expose a large contiguous range
   up front (Docker supports a range: `-p 20000-40000:20000-40000/udp`;
   Kubernetes needs either `hostNetwork` scoped to a node-level reserved
   range, or enumerating `hostPort` per container — there's no native range
   syntax in a Pod spec) and only actually `bind()` the subset needed at
   any given time via `wayhouse`'s own `bind: "host:lo-hi"` range listener. This
   is the same shape **Agones** (the standard Kubernetes game-server
   operator) uses for its `GameServer` pods — `hostNetwork: true` plus one
   port pulled from a node-level reserved pool — because Kubernetes
   Services can't do dynamic port publishing either; `docs/08`'s roadmap
   entry for the range-bind feature draws this comparison explicitly.

Host networking is the simpler default when the proxy doesn't need
network-namespace isolation from other workloads on the node (usually
true for an edge process). Reach for the reserved-range pattern when
isolation is a hard requirement, or when running many `wayhouse` instances per
node and the orchestrator should own port allocation/collision-avoidance
across them.

## `--tunnel-*` (phase 14): `CAP_NET_ADMIN` and `/dev/net/tun`

`wayhouse --tunnel-*` and `wayhouse-agent` create a real WireGuard interface
(`docs/11`) — kernel-netlink by default, falling back to `boringtun`
userspace with `--tunnel-userspace`/`--userspace`. **Both backends need
`CAP_NET_ADMIN`**, and the userspace fallback additionally needs
`/dev/net/tun` present in the container. Grant them explicitly:

```
docker run --user 0 --cap-add=NET_ADMIN --device=/dev/net/tun ...
```

`--user 0` matters for the nonroot images in `deploy/`: Docker and Kubernetes put an
added capability only in a *non-root* process's bounding set, so uid 65532 would still
get `EPERM` creating the interface.

```yaml
securityContext:
  runAsUser: 0
  runAsNonRoot: false
  capabilities:
    add: ["NET_ADMIN"]
volumes:
  - name: tun
    hostPath: { path: /dev/net/tun }
```

This is a real, already-verified requirement, not a guess: `docs/08`
phase 14 slice 6's end-to-end verification ran in exactly this
container shape (`crates/wayhouse-agent` + `wayhouse --tunnel-*` in Docker
containers with `NET_ADMIN` + `/dev/net/tun`) because the project's own
dev sandbox lacked `CAP_NET_ADMIN` even in the bounding set. A container
without both of these fails loud at interface bring-up, before any
listener binds or backend source is built — it does not silently run
without tunnel support.

The CI-runnable `make tunnel-e2e` (rootless network namespaces,
`docs/superpowers/specs/2026-10-01-tunnel-e2e-design.md`) verifies the tunnel
*logic* with the same binaries and flags, but it does not exercise container
capabilities — `NET_ADMIN` and `/dev/net/tun` as documented above stay the
deployment requirement.

## Authentication at startup

Every fleet API is open when no token is configured. Startup now enforces:

- **Non-loopback bind needs auth.** `wayhouse-controller`, `wayhouse-aggregator` and `wayhouse-ui`
  (`--listen`) and `wayhouse`'s admin API (`settings.admin.listen`) refuse to start on a
  non-loopback address without a token (`--auth-token`, `--ui-password` /
  `--users-file`, `settings.admin.auth_token`). If the network boundary really is your
  only control, pass `--insecure-no-auth`; it logs a warning instead.
- **HA needs `--ha-token`.** `wayhouse-controller --ha-peers` / `--ha-join` refuse to start
  without it, with no opt-out: `/raft/*` and `/admin/ha/members` would otherwise accept
  anyone. HA peers should use `https://` URLs (see below), since the token travels in
  each request.
- **Minimum secret length: 16 bytes** for `--auth-token`, `--ha-token`, `--ui-password`,
  `settings.admin.auth_token` and `settings.gossip.psk`. Generate one with
  `openssl rand -hex 32`. The outbound tokens (`--controller-token` etc., which must
  match the remote side) are not checked.

## Controller tokens: admin, proxy, agent

The controller separates three credentials, so a compromised edge box cannot administer the fleet:

- `--auth-token` is the admin token: config writes, secrets, plugins, ACME, the UI and operators.
- `--proxy-token` is what `wayhouse` proxies present (`--controller-token`,
  `--tunnel-controller-token`). It unlocks only what a proxy calls: `GET /config`,
  `GET /config/subscribe`, `GET /secrets/{name}/sealed`, `GET /plugin-routes/subscribe`,
  `GET /intent/subscribe`, `GET /peers/subscribe`, `GET /peers/{name}` (a tunnel backend's address), `GET /features/overrides` (live feature
  overrides) and `POST /proxy-peers`.
- `--agent-token` is what `wayhouse-agent` presents (`--controller-token`). It unlocks only
  `POST /peers` and `GET /proxy-peers/subscribe`. An origin that enrolled holds a token of its
  own instead (see "Enrolling an origin agent"), which also unlocks `GET /peers/{name}/psk` for
  that name only.

Setting `--auth-token` requires both of the others, and all three must differ; the controller
refuses to start otherwise. The admin token still works on every route (the UI, operators and a
slave controller's `--parent-token` use it). Under HA every replica takes the same three flags.
`wayhouse-certsync` reads `/acme/bundles` (private keys) and so holds the admin token.
Known limit: all proxies share one proxy token, so one compromised proxy can re-register another
proxy's tunnel record with `POST /proxy-peers`. Agents that enrolled have their own tokens (below);
an agent on the shared token can still re-register any origin that did not enroll (#581).

## Enrolling an origin agent with a one-time code

Instead of copying `--agent-token` and a name onto the origin host, mint a code on the controller
and redeem it there. The controller needs a secret key (`--secret-key-file`); the code travels
over TLS that the redeemer pins.

```sh
# On an admin machine: a 15-minute, single-use code for an origin named home-nas.
curl -sS -X POST "$CONTROLLER/enrollments" -H "Authorization: Bearer $(cat admin-token)" \
  -d '{"name": "home-nas"}'                      # {"id":"..","code":"..","expires_at":..}
# On the origin host (the code from a file, the environment, or typed at the prompt):
wayhouse-agent enroll --controller-url https://controller.example:9901 \
  --pin sha256:<leaf fingerprint> --code-file ./code
wayhouse-agent --data-dir wayhouse-agent-data --backends :25565
```

`enroll` writes `enrollment.json` (controller URL and the origin's name), `agent-token` (when the
controller has one) and `tunnel-psk` into `--data-dir` (0700, files 0600). Afterwards the agent
needs no `--controller-url`, `--name`, token or key flag. The controller makes a fresh WireGuard
preshared key for the origin, stores it sealed as `tunnel-psk-<hash of the name>`, and every proxy
(which needs `--secret-key-file`) fetches and applies it, so the tunnel's handshake is mixed
with a key an eavesdropper who records it today cannot recover later.

- Pin with `--pin sha256:<hex>` (the controller's leaf certificate) or `--controller-ca <pem>`.
  `https://` is required; plain `http://` is accepted only to a loopback host.
- A code is stored as a hash only. It is single use (`"uses"` raises that), expires after
  `ttl_secs` (default 900, at most 7 days), and `DELETE /enrollments/{id}` revokes it.
  `GET /enrollments` lists codes and their state, never the code.
- `POST /enroll` is the one unauthenticated write route: 1 KiB body, attempts limited per
  address and per code id, and every refusal looks the same.
- With `--auth-token` on the controller, `agent-token` is a token for this origin alone, not the
  shared `--agent-token`: it registers only this name and reads only this origin's key. Re-enrolling
  a name replaces its token (the old one stops working), starts the key over at generation 0 and
  removes every other generation; the agent must be restarted with the new files.
- Rotate the key without re-enrolling: `curl -X POST "$CONTROLLER/peers/home-nas/psk/rotate" -H
  "Authorization: Bearer $(cat admin-token)"` (`202`, then the agent picks it up within one
  `--register-interval-sec`; `409` while a rotation is waiting). Details in
  [docs/11](11-backend-transport.md#rotating-the-key). The UI button follows.
- Upgrade every proxy to 0.4.0 before enrolling an origin: an older proxy ignores the key and
  its handshake with the origin then fails.

### Enrolling a proxy, an aggregator or a UI

The same code works for the other node binaries. `POST /enrollments` takes a `role`
(`agent` by default, `proxy`, `aggregator`, `ui`, and `controller`, see "Adding a controller replica by code" below) and the redeem returns exactly that role's
files, named after the credentials the binary already reads (`$CREDENTIALS_DIRECTORY/<name>`):

| Role | Binary | Files written to `--data-dir` | Minted with |
|---|---|---|---|
| `agent` | `wayhouse-agent` | `agent-token`, `tunnel-psk` | admin token |
| `proxy` | `wayhouse` | `controller-token` (the proxy token), `aggregator-token` (the ingest token, if the controller has one), `secret.keys` (the cluster keyring) | admin token **and step-up secret** |
| `aggregator` | `wayhouse-aggregator` | `ingest-token`, and `auth-token` (made on the node, never sent) | admin token |
| `ui` | `wayhouse-ui` | `controller-token` (the **admin token**) | admin token **and step-up secret** |

```sh
curl -sS -X POST "$CONTROLLER/enrollments" -H "Authorization: Bearer $(cat admin-token)" \
  -d '{"role": "proxy", "name": "edge-1", "step_up": "'"$(cat step-up)"'"}'
sudo wayhouse install --systemd --controller-url https://controller.example:9901 \
  --pin sha256:<leaf fingerprint> --code-file ./code -- --aggregator https://agg.example:9902
```

- The keyring and the admin token are the crown jewels, so a `proxy` or `ui` code is only minted
  with the **step-up secret** in the body (the controller's `--enroll-step-up-file`,
  `$CREDENTIALS_DIRECTORY/enroll-step-up` or `$WAYHOUSE_ENROLL_STEP_UP`: at least 16 bytes,
  different from every token). Without that flag those roles cannot be minted. Wrong guesses are
  limited to five a minute. Limits of this gate: it protects against a leaked admin token, not
  against a compromised UI host (the UI sees the secret whenever the operator types it) or
  controller host (which already holds the keyring). The wrong-guess bucket is shared by all
  callers, so someone with only the admin token can use it up and block the operator for a
  minute, and under HA each replica counts separately. `proxy` and `ui` codes live at most 24
  hours and a `ui` code is single use. Every mint, refused mint, redeem and revoke is logged with the actor
  (the `X-Actor` header the UI sets), never with a secret.
- The controller needs `--ingest-token-file` (the same secret the aggregator and proxies use) to
  mint an `aggregator` code and to give proxies their aggregator token. An aggregator tier's
  `parent_url` can be set at mint time and is written to `enrollment.json`.
- A code redeems only as the role it was minted for. `enroll` refuses a `--data-dir` that is
  already enrolled.
- `enroll` (any binary) writes the files and prints how to run the node. `install --systemd`
  (needs root) enrolls into `/etc/wayhouse/<role>` (the agent: `/var/lib/wayhouse-agent`), then
  writes `wayhouse*.service` with `LoadCredential=` for every secret file and enables it. It
  refuses to replace an existing unit or enrollment. Flags after `--` go to the binary;
  `--credential NAME=PATH` loads a further secret (the UI's `ui-password`).
- The UI also needs `--aggregator-token` and `--ui-password`, and the aggregator needs
  `--instance-token`: enrollment does not deliver them (the controller does not hold them).
- Running without systemd: `CREDENTIALS_DIRECTORY=<data-dir> wayhouse --secret-key-file <data-dir>/secret.keys ...`
  reads the same files.

### Adding a controller replica by code

A second or third controller joins a running HA group with one command, and the new host never
holds an admin token to do it. The group must run HA already (`--ha-peers` on the first
controller; a standalone controller cannot mint this code) and have `--auth-token`, a secret key
(`--secret-key-file`) and `--enroll-step-up-file`.

```sh
# On anything that can reach the cluster: mint a code (admin token and step-up secret).
curl -sS -X POST "$CONTROLLER/enrollments" -H "Authorization: Bearer $(cat admin-token)" \
  -d '{"role": "controller", "name": "ctl-4", "step_up": "'"$(cat step-up)"'"}'
# -> {"code": "...", "node_id": 4, ...}

# On the new host (root; the cluster needs its own TLS certificate here, see "HA replicas over TLS"):
sudo wayhouse-controller install --systemd --controller-url https://ctl-1.internal:9901 \
  --pin sha256:<leaf fingerprint> --code-file ./code --advertise https://ctl-4.internal:9901 \
  --credential tls-key=/etc/wayhouse/ctl-4.key \
  -- --listen 0.0.0.0:9901 --tls-cert /etc/wayhouse/ctl-4.pem --ca-file /etc/wayhouse/ca.pem
# The unit runs as a dynamic user. `--credential tls-key=PATH` has systemd hand it the key (kept
# root-only) and adds `--tls-key=%d/tls-key` itself; do not also pass --tls-key after `--`.
```

What happens, in order:

1. The code is minted for node id **N+1** (above every member and every other controller code; two
   codes minted at once never share an id, because the replicated store refuses a second code
   with the same id). The answer carries `node_id`.
2. `enroll` redeems the code over the pinned TLS connection and writes, 0600 in
   `/etc/wayhouse/controller`: `ha-token`, `secret.keys` (the keyring), `auth-token` (the **admin**
   token), `proxy-token`, `agent-token`, `ingest-token` and `enroll-step-up` (each only if the
   first controller has one) and `ha-join-proof`, plus `enrollment.json` with the node id and the
   `--advertise` address. A replica serves the same API, so it holds every token the first
   controller holds; because the **Raft token is cluster-wide**, a replica holds the cluster's Raft
   credential, which is the same trust level as the keyring.
3. The unit starts `wayhouse-controller --ha-join --ha-node-id=N --ha-advertise=... --ha-join-via=...`
   with every secret as a `LoadCredential=` file. The new node answers `/raft/whoami` and asks
   `--ha-join-via` for `POST /enroll/ha-join {proof, addr}` until it is added.
4. The cluster runs the same member add as `POST /admin/ha/members`: the leader asks the node at
   the advertised address who it is (`/raft/whoami`, must answer as node N with an empty log),
   adds it as a learner, waits for it to catch up and promotes it. A follower relays the request to
   the leader.

Failure and retry:

- The join proof is a second secret, separate from the code (which is spent at the redeem). It is
  valid for one hour after the redeem, lives only in `ha-join-proof` on the node and, hashed, in
  the replicated store, and is useless once the node is a voter. The node asks again every few
  seconds (backing off to a minute) and logs the leader's reason each time: an address nothing
  answers on (`did not answer /raft/whoami`), a node that answers as another id, a log that is not
  empty, another membership change in progress. None of those uses the code up. A `401` (proof
  expired, revoked or wrong) ends the asking: remove the unit and the data directory, mint a new code and enroll again.
- `GET /enrollments` shows the code as `pending`, `joining` (redeemed, not yet a voter) or
  `joined`.
- `POST /enroll/ha-join` is unauthenticated like `POST /enroll` and guarded the same way (1 KiB
  body, limits per address and per code, one answer for every wrong proof). After a valid proof the
  leader sends the Raft token to the address in the request, so keep `ha-join-proof` as private as
  the other files.
- Roll out every replica to the build with this feature before minting a controller code
  (the UI data level is now 9; an older replica refuses the write).
- Not delivered: the replica's own TLS certificate and `--ca-file`. Give them after `--`.
  `--tunnel-network` comes from the first controller's flag.

### One code for a controller StatefulSet (pool code)

A controller code with `uses` 2 to 16 and a `name_prefix` is a **pool**: each pod redeems it under
its own name (`ctl-0`, `ctl-1`, ...) and gets its own node id and join proof, so one code serves a
whole StatefulSet and scaling up needs no new code (until the uses or the TTL run out).

```sh
curl -sS -X POST "$CONTROLLER/enrollments" -H "Authorization: Bearer $(cat admin-token)" \
  -d '{"role":"controller","name":"ctl-pool","name_prefix":"ctl-","uses":3,"ttl_secs":3600,"step_up":"'"$(cat step-up)"'"}'
```

- The node id is chosen when the pod redeems, inside the replicated store: one above every Raft
  member and every enrollment record, spent or not, so pods redeeming at once on different replicas
  never share an id. An id stays taken while its member or its record exists; removing the highest member and revoking its record frees it. The redeem writes a child record (the pod's name, its
  node id, its join proof hash) that `GET /enrollments` lists like a single-use controller code
  (`joining`, then `joined`); the pool itself shows `pending` and then `used`.
- **A name enrolls once.** A second redeem for a name that already has a replica record is refused
  (the usual `401`), even with uses left. A pod that keeps its data volume never redeems again; a pod
  that lost its volume is replaced deliberately: remove the member (`DELETE /admin/ha/members/{id}`),
  revoke its record (`DELETE /enrollments/{child id}`), then it can redeem again under the same name.
  The Raft data directory therefore must be a PersistentVolumeClaim.
- A pod stuck on `401` after a redeem answer got lost, or because a stranger enrolled its name first: open
  `GET /enrollments`, find the child with that name (`joining` and not yours, or `joining` with no member in
  `GET /admin/ha/members`) and revoke it (`DELETE /enrollments/{child id}`); a child that never joined has
  no member to remove. Then the pod redeems again.
- Whoever holds the pool code can enroll up to its remaining uses under any name with the prefix, and
  each of those gets the cluster's tokens, the keyring and the Raft token. Mint it only when the pods
  are about to start, with a short TTL (at most 24 hours), and revoke it afterwards
  (`DELETE /enrollments/{pool id}`); revoking does not undo replicas already enrolled.
- The pod starts with `--ha-join --ha-node-id-file=<enroll dir>/ha-node-id` (the file `enroll` writes beside
  `enrollment.json` for a replica) so its id need not be on the command line; `--ha-node-id` still works.
- **On Kubernetes**: `kubectl apply -k deploy/k8s/overlays/ha-enrolled` runs one bootstrap controller
  (`wayhouse-controller-0`, a group of one, on the shared Secrets) and a `wayhouse-controller-replica`
  StatefulSet whose init container redeems the pool code from the `wayhouse-enroll-code` Secret as
  `$(POD_NAME)` and keeps the result on a volume claim (a restart spends no use). Create first the
  Secrets `wayhouse-enroll-step-up` (key `step-up`, any 16+ character secret; `init` does not make it)
  and, after the bootstrap controller is up, mint the pool with `name_prefix`
  `wayhouse-controller-replica-` and `uses` at least the replica count, then
  `kubectl create secret generic wayhouse-enroll-code --from-file=code=...`. Scaling later needs a fresh
  code with uses left. Not validated on a real cluster (CI renders and lints it only).
- Needs the same group setup as a single-use controller code (HA, `--auth-token`, a secret key and
  `--enroll-step-up-file`). Roll every replica to this build first (the UI data level is 9).

### Per-node enrollment on Kubernetes

One multi-use code can enroll a whole DaemonSet. Mint it with `name_prefix` (an agent or proxy
code, the prefix must end in `-`) and `uses` (up to 1000, at least the node count, plus one for each
proxy pod that is recreated); each pod then redeems it under its own name, which must start with the
prefix and is at most 64 characters (longer is refused as "name too long"). An agent gets its own
token and tunnel key; proxies share the proxy and ingest tokens and the keyring, so the name only
tells them apart in the registry. A code without `name_prefix` refuses a name, and a prefix code
refuses a missing one or one outside the prefix, before a use is spent. A prefix code also refuses
(409) a name that already holds a token or tunnel key, so anyone who can read the code cannot take
over an enrolled origin; an agent that lost its files must be deleted from the controller (the UI's
"Tunnel addresses", or `DELETE /peers/{name}`) before it enrolls under that name again. Refused redeems still count
against the limit of 5 attempts a minute per code (10 per address), so a wrong name in a
DaemonSet that restarts in a loop locks the code out for a minute at a time.

```sh
curl -sS -X POST "$CONTROLLER/enrollments" -H "Authorization: Bearer $(cat admin-token)" \
  -H 'Content-Type: application/json' \
  -d '{"role":"agent","name":"k8s-agents","name_prefix":"agent-","uses":20,"ttl_secs":86400}' \
  | jq -r .code > code
kubectl -n wayhouse create secret generic wayhouse-enroll-code-agent --from-file=code=code && rm code
# The proxy DaemonSet needs its own code (role proxy, prefix k8s-, step-up secret, at most 24 hours),
# in its own Secret: a code has one role, so one Secret cannot serve both DaemonSets.
kubectl -n wayhouse create secret generic wayhouse-enroll-code-proxy --from-file=code=proxy-code && rm proxy-code
kubectl apply -k deploy/k8s/overlays/enrolled
```

The `enrolled` overlay adds two components: `components/enroll` makes the proxy DaemonSet redeem the
code in an init container as `k8s-<node name>` (mint that one with `"name_prefix":"k8s-"`, role
`proxy`, which also needs the step-up secret and lasts at most 24 hours) and reads its tokens from
an emptyDir instead of the shared `wayhouse-tokens` Secret; `components/agent` adds a `wayhouse-agent`
DaemonSet that enrolls as `agent-<node name>` (edit `--backends` in it). The init container runs
`enroll --skip-if-enrolled`, so a pod that starts again where its files still are spends no use.
The agent keeps its files on the node (a `hostPath`), so a recreated agent pod and its tunnel key
survive. The proxy runs as a non-root user and its files live in an emptyDir, which is gone with
the pod: **a recreated proxy pod redeems the code again**, so it needs a code that is still valid.

- The codes are not in any manifest: you create the `wayhouse-enroll-code-proxy` and
  `wayhouse-enroll-code-agent` Secrets (the controller pool code below keeps `wayhouse-enroll-code`), and `deploy/lint.sh`
  fails a render that defines it, passes `--code=`, or mounts the shared tokens on the enrolled proxy.
- A code stops working at its expiry whatever uses are left (`ttl_secs`). Keep the Secret in place
  while proxy pods can be recreated, and when you roll, add or replace nodes, mint a fresh code
  and update the Secret first (a proxy code lasts at most 24 hours, so this is a routine, not a
  one-off). A recreated proxy pod with an expired code stays in `Init` until you do. Each redeem
  also uses one of the code's uses, so mint with room to spare.
- Anyone who holds the code can enroll under any name with the prefix, including the name of an
  existing node, which replaces that node's token (and an agent's tunnel key). Keep the prefix
  specific and the Secret readable by the DaemonSets only.
- A controller StatefulSet uses a **pool code** instead, see "One code for a controller StatefulSet"
  below. The shipped HA overlay still uses static peers and the shared token Secrets.

### Adding a node from the UI

**Add node** (System group, admin role) mints a code and writes the command for you. Pick the role,
a name, and a target: systemd, `docker run`, Docker Compose or Kubernetes (for Kubernetes pods that enroll
by themselves see "Per-node enrollment on Kubernetes" below, the page does not mint those yet). Fill in the controller URL and the certificate fingerprint
`wayhouse-controller init` printed. The page then shows the steps once, each with a Copy button.

- The code lives in the page's memory only: never in browser storage or a URL, never logged by the
  UI, and gone when you press Done or leave the page. The UI's `/api/enrollments` routes are
  admin-only and answer `Cache-Control: no-store`.
- A `proxy` or `ui` code needs the controller's step-up secret, typed for every mint. The UI sends it
  with that one request and clears the field, so it is neither stored nor logged.
- "Keep the code out of the command" produces commands without the code; you paste it when the
  node asks, so it stays out of shell history and process lists.
- The list under the form shows every code (pending, used or expired) and revokes any of them.
  On **Tunnel addresses**, an origin row has **Re-enroll**, which opens the form pre-filled.
- A **Controller replica** code (systemd only) needs the HA group's step-up secret and the new node's
  advertise address. The page shows the Raft node id the replica gets, and the list shows the code as
  `joining` until the cluster has added it, then `joined`. The replica's own certificate, key and
  the cluster CA go in the paths the command names (change them as needed).
- **Controller replica** with **Kubernetes** mints a **pool code** (2 to 16 uses, name prefix
  `wayhouse-controller-replica-`, step-up secret required, 24 hours at most) for the `ha-enrolled`
  overlay. The page asks for no URL, fingerprint or advertise address (the overlay's init container
  knows them) and shows three steps: create the `wayhouse-enroll-step-up` Secret by hand (it is not
  made for you), store the code as the `wayhouse-enroll-code` Secret, apply the overlay and scale the
  replica StatefulSet to at most the code's uses. Revoke the pool in the list afterwards.

## TLS for the fleet services

Without the settings below, `wayhouse-controller`, `wayhouse-aggregator`, `wayhouse-ui` and `wayhouse`'s
admin API serve **plain HTTP**. Run as-is across a network, that exposes:

- the `--auth-token` bearer token on every request, and the `/admin/adopt` calls;
- the full config text, on `GET /config` and the SSE `GET /config/subscribe`;
- every origin's registration (WireGuard public key, public endpoint, fronted
  backend addresses) on the `/peers*` and `/proxy-peers*` routes.

Two ways to encrypt it: **native TLS** (below), or a **reverse proxy
you run that terminates TLS**, with the controller listening only on loopback or a
private network (`--listen 127.0.0.1:9901`, or a private bridge/pod network). The
aggregator, the UI and `wayhouse`'s admin API serve native TLS the same way.

### Native TLS

```sh
wayhouse-controller --listen 0.0.0.0:8443 \
  --tls-cert /etc/wayhouse/tls/fullchain.pem --tls-key /etc/wayhouse/tls/privkey.pem
```

- `--tls-cert` is a PEM chain, leaf first (a Let's Encrypt `fullchain.pem` works);
  `--tls-key` a PEM private key (PKCS#8, SEC1 or PKCS#1). Both or neither. With them,
  `--listen` speaks **HTTPS only** — every route, including the SSE streams.
- **Renewal without a restart:** both files are checked every 30 s and a changed pair is
  swapped in for new connections. A broken or half-written pair (no certificate, no
  key, key not matching the certificate) is logged and the current certificate stays in
  service until a good pair appears. Bad files at **startup** stop the controller with
  an error naming the file.
- Clients use `https://` URLs; with a private CA they add `--ca-file`. HA replicas can
  each serve native TLS and name their peers `--ha-peers 1=https://ctl-1:8443,…`
  (plus `--ca-file`), with no terminator at all.
- TLS handshakes run in their own tasks, so a client that connects and stalls cannot
  hold up others. A client must send its ClientHello within 3 s and finish the
  handshake within 10 s. At most 16 handshakes may be pending per source (an IPv4
  address or an IPv6 /64); more are closed at once. At 512 pending handshakes in total a
  new connection is still accepted and the oldest pending handshake is dropped, so a
  flood of idle connects cannot lock real clients out. A source may also open at most 20
  new connections a second on average (bursts of 64), so it cannot cycle connects under
  its pending cap. A limit being hit logs a warning (at most once a minute) and
  counts into `wayhouse_tls_handshakes_refused_total` / `wayhouse_tls_handshakes_evicted_total` on
  every binary's `/metrics` (docs/06). The limits are flags: `--tls-max-pending` (512),
  `--tls-max-pending-per-source` (16), `--tls-new-per-source-per-sec` (20; `0` turns the
  rate limit off) and `--tls-new-per-source-burst` (64); raise the per-source ones for
  a fleet that reaches the service from behind one NAT. No client certificates (mTLS).
- **`wayhouse-aggregator`** takes the same two flags with the same behaviour. Instances then
  push to `--aggregator https://…` (plus `--ca-file` for a private CA), and the same
  goes for a child tier's `--parent-url` and `wayhouse-ui --aggregator-url`.
- **`wayhouse-ui`** takes the same two flags. Serving HTTPS itself, it marks the session
  cookie `Secure` (without the flags it doesn't — a browser drops a `Secure` cookie on
  `http://` and login would loop). The live view's WebSocket works over HTTP/2 too:
  a browser on h2 opens it as an extended `CONNECT`, which the UI accepts (and the
  session cookie counts in whichever `cookie` header h2 splits it into). Plain
  HTTP is not redirected; serve only HTTPS on the port browsers use.
- **`wayhouse`'s admin API** is configured in the YAML, next to `listen`:
  `settings.admin.tls: { cert: <chain.pem>, key: <key.pem> }` (both required, plus the
  optional handshake limits `max_pending`, `max_pending_per_source`,
  `new_per_source_per_sec`, `new_per_source_burst`, same meaning as the flags above;
  startup-only like `listen`; the files renew like the flags above; `wayhouse --check`
  loads them). The admin URL `wayhouse` reports to the aggregator then becomes
  `https://<settings.admin.listen>`, so the certificate needs that address as a SAN
  (an IP SAN for an IP `listen`), and the aggregator takes `--ca-file` for a private
  CA. As before, the reported URL is the literal `listen` address — a wildcard bind
  (`0.0.0.0`) is not reachable as an admin URL, TLS or not. With `--controller` one
  YAML reaches every instance, so put each host's own certificate at the same path.
  Health probes against the admin port must then use HTTPS (Kubernetes:
  `httpGet.scheme: HTTPS`).

Verified by `wayhouse-fleet-tests`: `controller_native_tls.rs` (`wayhouse --check` fails on
`UnknownIssuer` without `--ca-file` and passes with it; `/config/subscribe` streams over
TLS; `--tls-cert` alone is refused), `aggregator_native_tls.rs` (a `wayhouse` pushes to an
`https://` aggregator, read back over HTTPS), `ui_native_tls.rs` (login over HTTPS
sets a `Secure` cookie that unlocks `/ui/session`), `admin_native_tls.rs` (an
aggregator fans an intent verb out to an `https://` admin API; `wayhouse --check` names a
bad `settings.admin.tls.cert`) and `ha_tls.rs`
`three_replicas_replicate_over_native_tls`; certificate loading, renewal and the
listener are unit-tested in `crates/wayhouse-http/tests/tls_{certs,server}.rs`.

### Proxy configuration

Four routes are long-lived server-sent-event streams — `/config/subscribe`,
`/peers/subscribe`, `/proxy-peers/subscribe` and `/intent/subscribe`. The controller
sends keep-alives, but a proxy that buffers responses or enforces a short read
timeout will stall or drop them, so those must be off/long.

Caddy (certificates from Let's Encrypt automatically):

```
controller.example.com {
    reverse_proxy 127.0.0.1:9901 {
        flush_interval -1        # stream SSE immediately
    }
}
```

nginx:

```nginx
server {
    listen 443 ssl;
    http2 on;
    server_name controller.example.com;
    ssl_certificate     /etc/letsencrypt/live/controller.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/controller.example.com/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:9901;
        proxy_http_version 1.1;
        proxy_set_header Connection "";
        proxy_buffering off;      # SSE
        proxy_read_timeout 1h;    # idle SSE streams
    }
}
```

> These two snippets are **not tested in this repository** (CI has neither proxy, and
> an end-to-end https test would need a certificate the clients trust — see the limits
> below). Check them against your proxy's version.

In Kubernetes the equivalent is an Ingress (or Gateway) with TLS in front of the
`wayhouse-controller` Service; make sure its response buffering and timeouts allow SSE.

### Pointing the clients at it

Every HTTP client in the fleet uses `reqwest` with rustls and builds its requests from
the base URL as given — no code forces a scheme — so these take an `https://` base URL:
`wayhouse --controller`, `wayhouse --tunnel-controller-url`, `wayhouse-agent --controller-url`,
`wayhouse-ui --controller-url` / `--aggregator-url`, `wayhouse-controller --parent-url`, and the
`parent_url` in a `POST /admin/adopt` body.
To check it works:

```sh
curl -fsS https://controller.example.com/healthz
wayhouse --check --controller https://controller.example.com --controller-token "$TOKEN"
```

**Private or self-signed CA.** Every binary (`wayhouse`, `wayhouse-agent`, `wayhouse-controller`,
`wayhouse-aggregator`, `wayhouse-ui`) takes `--ca-file <PATH>`: a PEM file with one or more CA
certificates to trust for **all** of that process's outbound HTTPS, in addition to the
built-in Mozilla roots (it never replaces them, so public endpoints keep working).
Non-certificate PEM sections (a private key pasted into the same file) are ignored. A
missing or unreadable file, one with no certificates, or a malformed certificate stops
the process at startup with an error naming the file. The file is read **once, at
startup**: `SIGHUP` / a config reload does not re-read it, so restart the process after
changing it (e.g. when rotating the CA).

```sh
wayhouse --check --controller https://controller.internal:8443 --ca-file /etc/wayhouse/ca.pem
```

This exact shape — `wayhouse --check` against a real `wayhouse-controller` behind a TLS
terminator whose certificate a private CA signed, failing without `--ca-file` and
passing with it — is the `wayhouse-fleet-tests` test `ca_file.rs`, run by `make check`.
The two proxy snippets above are still untested.

A TLS or connection failure is logged with its cause (e.g. `invalid peer certificate:
UnknownIssuer`), not just "error sending request" — that is usually the first sign a
`--ca-file` is missing or wrong.

### HA replicas over TLS

Put a TLS terminator in front of **each** replica's listener and give `--ha-peers`
base URLs instead of `host:port` (same list on every replica), plus `--ca-file` if the
terminators' certificates come from a private CA:

```sh
wayhouse-controller --listen 127.0.0.1:9901 --ha-node-id 1 --ca-file /etc/wayhouse/ca.pem \
  --ha-peers 1=https://ctl-1.internal:8443,2=https://ctl-2.internal:8443,3=https://ctl-3.internal:8443
```

Raft RPCs (`/raft/append`, `/raft/vote`, `/raft/snapshot`) and writes a follower
forwards to the leader then go over HTTPS. `id=host:port` still means plain HTTP, and
the two forms can be mixed. Raft carries the `--ha-token`, so a replica **refuses to start** with a
plain-HTTP peer beyond loopback (and `POST /admin/ha/members` refuses such an address with a `422`)
unless `--allow-insecure-controller` is set, for a cluster network that is private by construction
(the Kubernetes HA overlay does not need it: its peers are `https://`); the same flag covers a slave's `--parent-url`. An entry that is not `http(s)://host[:port]` (another
scheme, a path, a query, user info) stops the replica at startup naming the entry.
Verified by `wayhouse-fleet-tests` `ha_tls.rs`: three replicas that reach each other only
through private-CA terminators elect a leader, forward writes and all serve the last
one with `--ca-file`, and never elect a leader without it.

**Only at bootstrap.** `--ha-peers` is read when the cluster is first initialised;
after that each member's address lives in the replicated Raft membership. Changing
`--ha-peers` on an existing cluster does **not** change the addresses replicas use. To
move a running cluster to `https://` peers, change each member's address with a `PUT`
(see "HA membership" below).

### HA membership

Members are changed on a running cluster with the admin routes, gated by
`--auth-token` (pass it as a bearer token) and answered by any node, which forwards a
change to the leader. `X-Actor` is logged.

```sh
# A new or replacement node always starts empty with --ha-join (never --ha-peers: a
# node with --ha-peers calls initialize and could split the cluster).
wayhouse-controller --listen 0.0.0.0:7070 --ha-node-id 4 --ha-token "$HA_TOKEN" \
  --tunnel-network 10.60.0.0/24 --ha-join

curl -H "Authorization: Bearer $TOKEN" http://ctl-1:7070/admin/ha/members            # voters, learners, leader
curl -H "Authorization: Bearer $TOKEN" -d '{"id":4,"addr":"ctl-4:7070"}' \
  http://ctl-1:7070/admin/ha/members                                                # add (waits for catch-up)
curl -H "Authorization: Bearer $TOKEN" -X DELETE http://ctl-1:7070/admin/ha/members/2  # remove
curl -H "Authorization: Bearer $TOKEN" -X PUT -d '{"addr":"https://ctl-3.internal:8443"}' \
  http://ctl-1:7070/admin/ha/members/3                                              # change an address
```

An add answers once the new node has caught up, by log or, after the log was purged,
by snapshot; a large catch-up can take minutes (a forwarded add waits up to five).
Before an add or a `PUT` the leader asks the node at the given address who it is
(`/raft/whoami`, behind `--ha-token`) and refuses (`422`) unless it answers with that
node id, and an add also unless the node's log is empty (or it is already a learner of
this cluster): this keeps a node id from being pointed at another node's address, which
`openraft` warns can produce two leaders. Other answers: `409` the node is already a
voter, `404` unknown id, `422` removing the last voter, `503` the node is unreachable or
there is no leader. Removing the current leader is allowed. A joined node restarts with
`--ha-join` again. `--ha-join` and `--ha-peers` together, and `--tunnel-readdress` with
either, are refused at startup. Verified by `wayhouse-fleet-tests` `ha_tunnel_addresses.rs`
(join, remove the leader, move a node to a new port) and `ha_snapshots.rs` (catch-up by
snapshot after a purge).

### Upgrading a single controller to HA

A controller that already holds registrations (origins, proxies, addresses) from before
HA can become the first node of an HA cluster. Restart it with `--ha-node-id` and the
`--ha-peers` of the new cluster next to the other, empty nodes. Every node sets aside
pre-HA data it finds as `peers.pre-ha/`, `proxy-peers.pre-ha/` and
`tunnel-addresses.pre-ha/` under `--data-dir` (it never deletes them), and the cluster's
leader imports exactly one node's copy, whichever node wins the first election.

- The leader asks every voter what it holds (`/raft/whoami`) before initializing, so
  registry writes answer `503` until every voter has answered; a voter that is down
  delays the upgrade. `--ha-import-source <node-id>` (the same value on every node)
  waits only for that node; `none` imports nothing.
- When **more than one** node has pre-HA data (for example a pin-only `--ha-peers`
  deployment where each node kept its own registries) nothing is initialized: the leader
  logs an `ERROR` naming the nodes and their counts, and registry writes stay `503`
  until you restart the nodes with `--ha-import-source <node-id>`. The other nodes'
  set-aside data stays untouched (one `WARN` each).
- Imported registrations are re-written as new revisions that continue after the source's
  last revision number, so every edge's `since` cursor from the single-node days stays
  valid. Tombstones the old node wrote are carried over too (the entry lists the names
  whose newest log entry is a removal), so a subscribed edge that had not received one
  still drops the peer, and a registry with no live registration keeps its log head.
  `ImportContent` is a Raft log entry: a node on an older build applies an `Import`
  without those tombstones, so upgrade every node of the cluster before the first
  initialization.
- Clients' pinned addresses that used to collide across nodes now get `409`: uniqueness
  is cluster-wide, where it was per node before.
- Snapshots persisted by a build older than this feature (before 2026-10-03) are not
  supported: a follower refuses to install one (`ERROR` naming the snapshot format) rather
  than read it as empty registries and an empty book. Start such a node from empty
  storage, or let the cluster re-initialize as above.
- The registries compact their logs: each keeps one entry per name (its registration, or
  the tombstone of a removed one), so a registry log and the Raft snapshot stay bounded by
  the number of distinct names ever registered, not by how often they re-register.
  Revision numbers have gaps afterwards; a subscriber's `since` cursor is unaffected.

### Limits (read these before relying on it)

- **No system certificate store.** The clients trust the Mozilla root bundle compiled
  into the binary (`reqwest`'s `rustls-tls` / `webpki-roots`) plus `--ca-file`, **not**
  the OS store or `SSL_CERT_FILE`. For a private or internal CA — or a self-signed
  certificate — pass it with `--ca-file`. No client certificates (mTLS).
- **HA over TLS** (native TLS on each replica, or a terminator per replica; see "HA
  replicas over TLS"): bootstrap with `https://` peers, or move a running cluster with
  one `PUT /admin/ha/members/{id}` per member (see "HA membership"). With `host:port`
  peers, replica traffic is plain HTTP; then keep
  the replicas on a private network — `--ha-token` is a shared secret, not encryption.
  The controller logs a warning for each non-loopback `http://` peer.
- **Upgrading a gossip mesh** (the datagram format gained a MAC'd sender timestamp): upgrade all instances in a failure domain together. While old
  and new instances are mixed they cannot exchange membership or health, so the
  domain quorum derived from gossip is unreliable until the rollout finishes (local
  health checks keep working). Old-to-new datagrams are counted in
  `wayhouse_gossip_stale_rejected_total`. Instance clocks must also agree within 30 s.
- **The UI behind a proxy** sees plain HTTP, so its session cookie is `HttpOnly;
  SameSite=Lax` but **not** `Secure`: add it, redirect HTTP to HTTPS (and consider
  HSTS) at the proxy — or use the UI's native TLS, which sets `Secure` itself.
- **A TLS proxy is a trust boundary.** It sees every token and registration in the
  clear. Run it on a host you control — or use native TLS where it exists.

## Spreading a fleet across sites (`failure_domain`)

Use this when proxies and backends live in more than one site, provider or region. A
failure domain is a set of proxies that reach their backends over the same network path
(an availability zone, a region, a rack row; see [docs/10](10-distributed-control-plane.md)
"Failure domain"). Sharing "backend X is down" is only meaningful inside one domain, so
give each proxy the domain it actually sits in.

1. Name the domains after reachability, not ownership. Two proxies in `eu-west-1a` share
   opinions; a proxy in another region or provider gets its own name.
2. On every proxy, set `settings.failure_domain` and `settings.gossip` together (setting
   one alone fails `validate()`). `gossip.bind` is that proxy's own reachable UDP address
   (not `0.0.0.0`); `seeds` can be any subset of the domain's other proxies; `psk` is the
   same string on every proxy of the domain, at least 16 bytes.
3. Open the gossip UDP port between proxies of the same domain only. Do not peer across
   domains.
4. Keep `quorum_fraction` at the default 0.66 unless the domain has two or three proxies;
   it must stay above 0.5.

```yaml
# proxies in eu-west-1a (the proxies in us-east-1 use failure_domain: "us-east-1"
# and their own psk and seeds)
settings:
  failure_domain: "eu-west-1a"
  gossip:
    bind: "10.0.1.4:7946"
    seeds: ["10.0.1.5:7946", "10.0.1.6:7946"]
    psk: "a-long-random-string-per-domain"
```

What this does: a proxy marks a backend unhealthy on its own failed checks or when a
quorum of its domain reports it down. It only returns to healthy on its own successful
checks, and an operator `force-down` always wins. An empty or partitioned mesh falls back
to each proxy's own checks. Gossip carries health only, not load, and does not move
traffic between sites; to prefer a nearer backend use the pool balancer and weights
([docs/03](03-routing.md)).

## Certificates from the controller

With `--acme` the controller can issue the web front door's certificates; a gateway host
fetches them with `wayhouse-certsync`, which ships in the `wayhouse` image. See
[`acme.md`](acme.md#distribution-to-gateways) for the gateway token, the files it writes and
how to run it.

## Tunnel addressing

Tunnel-internal addressing (the WireGuard-side space between proxies and origins) is
not an open question any more: `wayhouse-controller --tunnel-network` allocates the
addresses, so nothing here hand-picks one. See `docs/11` "Address authority".

**Choosing the network.** Use an IPv6 unique local address (ULA) network by default:
generate a random `/48` once and take a `/64` from it, so it cannot clash with any
origin's own LAN or another deployment's tunnel:

```
printf 'fd%02x:%02x%02x:%02x%02x::/48\n' $(od -An -N5 -tu1 /dev/urandom)
# e.g. fd49:89c1:4b5e::/48 -> --tunnel-network fd49:89c1:4b5e:60::/64
```

Choose an IPv4 network (`/16` to `/30`, e.g. `10.60.0.0/16`) instead when a game server
binds `0.0.0.0` only: it cannot be reached on an IPv6 tunnel address. The family is
fixed per controller; changing it later needs `--tunnel-readdress` (docs/11).

**Containers.** The tunnel interface needs IPv6 enabled in its network namespace.
Docker starts containers on a network without IPv6 with
`net.ipv6.conf.all.disable_ipv6=1`; for a container with its own network namespace set
`sysctls: { net.ipv6.conf.all.disable_ipv6: "0" }` (Compose) or the equivalent
`securityContext.sysctls` entry (Kubernetes). With host networking, as in
`deploy/compose/compose.tunnel.yml` and the DaemonSet's `hostNetwork`, the host's own
setting applies and such a sysctl is refused, so the host must not have IPv6 disabled.

This document otherwise covers only the container/orchestrator-facing side of the
proxy's public listeners.
