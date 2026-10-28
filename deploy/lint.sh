#!/bin/sh
# Static checks on deploy/ that need Docker's CLI but no daemon (compose config
# only renders). Each check pins a failure that only shows up on a real
# `docker compose up` / cluster — see docs/superpowers/plans/2026-10-01-deploy.md.
set -eu
cd "$(dirname "$0")/.."
sh deploy/check-caddy-modules.sh
sh deploy/check-caddy-modules-selftest.sh
# Render the defaults: CI's deploy job exports BIN_SOURCE=prebuilt, which must not
# change what these checks see.
unset BIN_SOURCE
export WAYHOUSE_CONTROLLER_TOKEN=x WAYHOUSE_AGGREGATOR_TOKEN=x WAYHOUSE_INGEST_TOKEN=y WAYHOUSE_UI_PASSWORD=x \
       WAYHOUSE_TUNNEL_ENDPOINT=1.2.3.4:51820
C=deploy/compose

base=$(docker compose -f $C/docker-compose.yml config --format json)
tunnel=$(docker compose -f $C/docker-compose.yml -f $C/compose.tunnel.yml \
         --profile origin-demo config --format json)
web=$(docker compose -f $C/docker-compose.yml -f $C/compose.web.yml config --format json)
smoke_plan=$(make -n deploy-smoke)
# Every overlay must build; the Ruby below checks the rendered DaemonSets, not the source files.
if command -v kubectl >/dev/null 2>&1; then kz="kubectl kustomize"; else kz="kustomize build"; fi
for o in single ha web-gateway; do
  $kz deploy/k8s/overlays/$o > "${TMPDIR:-/tmp}/wayhouse-k8s-$o.yaml"
done
export K8S_RENDERED="${TMPDIR:-/tmp}/wayhouse-k8s"

BASE="$base" TUNNEL="$tunnel" WEB="$web" SMOKE_PLAN="$smoke_plan" ruby -rjson -ryaml -e '
errs = []
base   = JSON.parse(ENV["BASE"])["services"]
tunnel = JSON.parse(ENV["TUNNEL"])["services"]
web    = JSON.parse(ENV["WEB"])["services"]

# curl 8.11.1 (the pinned seed image) rejects `--opt=value`; values must be
# separate arguments or `seed` exits 2 and wayhouse never starts.
bad = base["seed"]["command"].grep(/\A--[a-z-]+=/)
errs << "seed passes --opt=value args to curl: #{bad.inspect}" unless bad.empty?

# wayhouse must report a unique instance name (default = admin listen addr).
errs << "compose wayhouse lacks --aggregator-instance" unless base["wayhouse"]["command"].any? { |a| a.start_with?("--aggregator-instance=") }
k8s = %w[single ha web-gateway].to_h { |o| [o, YAML.load_stream(File.read("#{ENV["K8S_RENDERED"]}-#{o}.yaml"))] }
k8s.each do |o, docs|
  ds = docs.find { |d| d["kind"] == "DaemonSet" && d.dig("metadata", "name") == "wayhouse" }
  c = ds["spec"]["template"]["spec"]["containers"][0]
  errs << "k8s #{o}: wayhouse DaemonSet lacks serviceAccountName wayhouse (base/45-wayhouse-rbac.yaml)" unless ds.dig("spec", "template", "spec", "serviceAccountName") == "wayhouse"
  errs << "k8s #{o}: wayhouse lacks --aggregator-instance=$(NODE_NAME)" unless c["args"].include?("--aggregator-instance=$(NODE_NAME)")
  errs << "k8s #{o}: wayhouse lacks NODE_NAME from spec.nodeName" unless c["env"].any? { |e| e["name"] == "NODE_NAME" && e.dig("valueFrom", "fieldRef", "fieldPath") == "spec.nodeName" }
end
# HA overlay: a 3-node StatefulSet replaces the Deployment, node ids come from the pod ordinal, and the peer token is wired.
ha = k8s["ha"]
errs << "k8s ha: the single-writer controller Deployment must be gone" if ha.any? { |d| d["kind"] == "Deployment" && d.dig("metadata", "name") == "wayhouse-controller" }
errs << "k8s ha: the shared controller PVC must be gone" if ha.any? { |d| d["kind"] == "PersistentVolumeClaim" }
sts = ha.find { |d| d["kind"] == "StatefulSet" && d.dig("metadata", "name") == "wayhouse-controller" }
errs << "k8s ha: no wayhouse-controller StatefulSet with 3 replicas" unless sts && sts["spec"]["replicas"] == 3
if sts
  a = sts["spec"]["template"]["spec"]["containers"][0]["args"]
  errs << "k8s ha: controller needs --ha-node-id=$(POD_INDEX), --ha-token and 3 --ha-peers" unless a.include?("--ha-node-id=$(POD_INDEX)") && a.include?("--ha-token=$(HA_TOKEN)") && a.any? { |x| x.start_with?("--ha-peers=") && x.split(",").size == 3 }
end
errs << "k8s single/web-gateway must keep the one-replica controller Deployment" unless %w[single web-gateway].all? { |o| k8s[o].any? { |d| d["kind"] == "Deployment" && d.dig("metadata", "name") == "wayhouse-controller" } }

# NET_ADMIN is only in the bounding set for a non-root user: tunnel services run as root.
%w[wayhouse agent].each { |s| errs << "tunnel #{s} must run as root (user: \"0\")" unless tunnel[s]["user"] == "0" }
# The agent must not clash with wayhouse on the shared host network.
errs << "tunnel agent needs its own --listen-port" unless tunnel["agent"]["command"].include?("--listen-port=51821")
errs << "tunnel agent should sit in the origin-demo profile" unless tunnel["agent"]["profiles"] == ["origin-demo"]

# A failing `up` must still dump logs + tear down, in a project that cannot clobber the demo.
plan = ENV["SMOKE_PLAN"]
errs << "deploy-smoke must use its own project (-p wayhouse-smoke)" unless plan.include?("-p wayhouse-smoke")
errs << "deploy-smoke must dump logs on any failure" unless plan.include?(" logs")
errs << "deploy-smoke runs `up` as its own recipe line: a failed up skips logs + down" if plan.lines.any? { |l| l =~ /\Adocker compose.* up / && !l.include?("|| rc=") }

# Release images must not honour WAYHOUSE_TEST_PROTOCOL_* (the cargo feature of the fleet tests,
# `test-protocol-override`): nothing that builds or ships an image may enable it.
%w[deploy/Dockerfile deploy/build-images.sh .github/workflows/release.yml .github/workflows/ci.yml].each do |f|
  errs << "#{f} enables test-protocol-override, which lets a release image fake its protocol version" if File.exist?(f) && File.read(f).include?("test-protocol-override")
end

# wayhouse image needs a 65532-owned /data (tunnel key file volume).
df = File.read("deploy/Dockerfile")
wayhouse_stage = df[/AS wayhouse\n.*?(?=\nFROM |\z)/m]
errs << "wayhouse image lacks a 65532-owned /data" unless wayhouse_stage.include?("--chown=65532:65532 /out/data /data")
minimal_stage = df[/AS wayhouse-minimal\n.*?(?=\nFROM |\z)/m]
errs << "wayhouse-minimal image lacks a 65532-owned /data" unless minimal_stage.to_s.include?("--chown=65532:65532 /out/data /data")

# Prebuilt mode (CI feeds binaries from the shared release build): the stage
# selector, the prebuilt stage, runtime stages reading from `bins`, and the
# compose/build-images plumbing must all agree.
errs << "Dockerfile lacks `ARG BIN_SOURCE=builder` before the first FROM" unless df =~ /\A(?:#[^\n]*\n|\n)*ARG BIN_SOURCE=builder\n/
errs << "Dockerfile lacks `FROM ${BIN_SOURCE} AS bins`" unless df.include?("FROM ${BIN_SOURCE} AS bins")
errs << "Dockerfile lacks a `prebuilt` stage" unless df =~ /^FROM \S+ AS prebuilt$/
errs << "a runtime stage still copies from builder directly" if df.split("# --- runtime targets").last.include?("--from=builder")
errs << "compose build must pass BIN_SOURCE" unless base["wayhouse"]["build"]["args"].is_a?(Hash) && base["wayhouse"]["build"]["args"]["BIN_SOURCE"] == "builder"
errs << "build-images.sh must pass --build-arg BIN_SOURCE" unless File.read("deploy/build-images.sh").include?("--build-arg BIN_SOURCE=")

# The tunnel override must use the address authority, not hand-picked addresses.
ctl_cmd = tunnel["controller"]["command"]
errs << "tunnel override: controller needs --tunnel-network=fd49:89c1:4b5e:60::/64 (IPv6 is the default)" unless ctl_cmd.include?("--tunnel-network=fd49:89c1:4b5e:60::/64")
# Both tunnel services share the host network namespace, where runc refuses
# net.* sysctls ("not allowed in host network namespace"): `up` would fail.
%w[wayhouse agent].each { |s| errs << "tunnel #{s} sets a net.* sysctl, which host networking refuses" if (tunnel[s]["sysctls"] || {}).keys.any? { |k| k.start_with?("net.") } }
errs << "tunnel override: controller lost the base flags" unless %w[--listen=0.0.0.0:9901 --data-dir=/data].all? { |f| ctl_cmd.include?(f) } && ctl_cmd.any? { |a| a.start_with?("--auth-token=") }
errs << "tunnel override: wayhouse must not hand-pick --tunnel-address" if tunnel["wayhouse"]["command"].any? { |a| a.start_with?("--tunnel-address") }
agent_cmd = tunnel["agent"]["command"]
errs << "tunnel override: agent must not hand-pick --address" if agent_cmd.any? { |a| a.start_with?("--address") }
errs << "tunnel override: agent backends should use the :port shorthand" unless agent_cmd.include?("--backends=:25565")

# Web front door: wayhouse reaches Caddy on the host loopback and web-sync reads wayhouse
# there, so all three share the host network; the sync runs from the wayhouse image and
# talks to Caddy over the shared socket volume.
%w[caddy web-sync].each { |n| errs << "web override: #{n} must use host networking" unless web[n]["network_mode"] == "host" }
errs << "web override: web-sync must run wayhouse-web-sync from the wayhouse image" unless web["web-sync"]["image"] == "wayhouse-deploy/wayhouse:local" && web["web-sync"]["entrypoint"] == ["/usr/local/bin/wayhouse-web-sync"]
errs << "web override: caddy and web-sync must share /run/caddy" unless [web["caddy"], web["web-sync"]].all? { |x| x["volumes"].any? { |v| v["target"] == "/run/caddy" && v["source"] == "caddy-run" } }
errs << "web override: caddy must drop all capabilities" unless web["caddy"]["cap_drop"] == ["ALL"]
gw = k8s["web-gateway"].find { |d| d["kind"] == "DaemonSet" && d.dig("metadata", "name") == "wayhouse-web" }
gwc = gw["spec"]["template"]["spec"]["containers"]
errs << "k8s web gateway must use hostNetwork (wayhouse reaches Caddy on the node loopback)" unless gw["spec"]["template"]["spec"]["hostNetwork"] == true
errs << "k8s web gateway probes must set host: 127.0.0.1 (hostNetwork pods probe the node IP; both targets listen on loopback)" unless gwc.flat_map { |c| %w[readinessProbe livenessProbe].map { |k| c[k] } }.compact.all? { |pr| (pr["httpGet"] || pr["tcpSocket"])["host"] == "127.0.0.1" }
errs << "k8s web gateway containers must drop all capabilities" unless gwc.all? { |c| c.dig("securityContext", "capabilities", "drop") == ["ALL"] }
errs << "k8s web gateway: web-sync must point at the socket Caddy serves" unless gwc.find { |c| c["name"] == "web-sync" }["args"].include?("--caddy-admin=/run/caddy/admin.sock")

# The Caddy image: pinned by version AND digest, no file capability left, base config shipped.
caddy_stage = df[/AS wayhouse-caddy\n.*?(?=\nFROM |\z)/m]
errs << "wayhouse-caddy must be pinned by `FROM caddy:<version>@sha256:...`" unless df =~ /^FROM caddy:\d+\.\d+\.\d+@sha256:[0-9a-f]{64} AS wayhouse-caddy$/
errs << "wayhouse-caddy must replace /usr/bin/caddy with the patched build (COPY --from=caddy-build, which also drops the base image file capability)" unless caddy_stage.include?("COPY --from=caddy-build --chmod=0755 /out/caddy /usr/bin/caddy")
errs << "wayhouse-caddy must run as 65532" unless caddy_stage.include?("USER 65532:65532")
errs << "wayhouse image must carry wayhouse-web-sync" unless wayhouse_stage.include?("/out/wayhouse-web-sync /usr/local/bin/wayhouse-web-sync")
errs << "wayhouse image must carry wayhouse-certsync" unless wayhouse_stage.include?("/out/wayhouse-certsync /usr/local/bin/wayhouse-certsync")

if errs.empty? then puts "deploy lint: ok" else warn errs.map { |e| "LINT FAIL: #{e}" }; exit 1 end
'
