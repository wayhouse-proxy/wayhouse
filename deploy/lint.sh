#!/bin/sh
# Static checks on deploy/ that need Docker's CLI but no daemon (compose config
# only renders). Each check pins a failure that only shows up on a real
# `docker compose up` / cluster — see docs/superpowers/plans/2026-10-01-deploy.md.
set -eu
cd "$(dirname "$0")/.."
sh deploy/check-caddy-modules.sh
sh deploy/check-caddy-modules-selftest.sh
# The example tokens must meet the services' 16-byte minimum, or the demo controller exits at start.
awk -F= '/^WAYHOUSE_[A-Z_]*(TOKEN|PASSWORD)=/ && length($2) < 16 { print "deploy/compose/.env.example: " $1 " is under 16 bytes"; bad = 1 } END { exit bad }' deploy/compose/.env.example
sh deploy/check-observability.sh
# Render the defaults: CI's deploy job exports BIN_SOURCE=prebuilt, which must not
# change what these checks see.
unset BIN_SOURCE
export WAYHOUSE_CONTROLLER_TOKEN=x WAYHOUSE_PROXY_TOKEN=p WAYHOUSE_AGENT_TOKEN=a WAYHOUSE_AGGREGATOR_TOKEN=x WAYHOUSE_INGEST_TOKEN=y WAYHOUSE_UI_PASSWORD=x \
       WAYHOUSE_TUNNEL_ENDPOINT=1.2.3.4:51820
C=deploy/compose

base=$(docker compose -f $C/docker-compose.yml config --format json)
tunnel=$(docker compose -f $C/docker-compose.yml -f $C/compose.tunnel.yml \
         --profile origin-demo config --format json)
web=$(docker compose -f $C/docker-compose.yml -f $C/compose.web.yml config --format json)
solo=$(docker compose -f $C/docker-compose.yml -f $C/compose.solo.yml config --format json)
solo_acme=$(docker compose -f $C/docker-compose.yml -f $C/compose.solo.yml -f $C/compose.solo-acme.yml config --format json)
smoke_plan=$(make -n deploy-smoke)
# Every overlay must build; the Ruby below checks the rendered DaemonSets, not the source files.
if command -v kubectl >/dev/null 2>&1; then kz="kubectl kustomize"; else kz="kustomize build"; fi
for o in single ha web-gateway; do
  $kz deploy/k8s/overlays/$o > "${TMPDIR:-/tmp}/wayhouse-k8s-$o.yaml"
done
export K8S_RENDERED="${TMPDIR:-/tmp}/wayhouse-k8s"

BASE="$base" TUNNEL="$tunnel" WEB="$web" SOLO="$solo" SOLO_ACME="$solo_acme" SMOKE_PLAN="$smoke_plan" ruby -rjson -ryaml -e '
errs = []
base   = JSON.parse(ENV["BASE"])["services"]
tunnel = JSON.parse(ENV["TUNNEL"])["services"]
web    = JSON.parse(ENV["WEB"])["services"]
solo   = JSON.parse(ENV["SOLO"])["services"]
solo_acme = JSON.parse(ENV["SOLO_ACME"])["services"]

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
# HA overlay: a 3-node StatefulSet replaces the Deployment, node ids come from the pod ordinal, and the peer token is wired (as a file).
ha = k8s["ha"]
errs << "k8s ha: the single-writer controller Deployment must be gone" if ha.any? { |d| d["kind"] == "Deployment" && d.dig("metadata", "name") == "wayhouse-controller" }
errs << "k8s ha: the shared controller PVC must be gone" if ha.any? { |d| d["kind"] == "PersistentVolumeClaim" }
sts = ha.find { |d| d["kind"] == "StatefulSet" && d.dig("metadata", "name") == "wayhouse-controller" }
errs << "k8s ha: no wayhouse-controller StatefulSet with 3 replicas" unless sts && sts["spec"]["replicas"] == 3
if sts
  a = sts["spec"]["template"]["spec"]["containers"][0]["args"]
  errs << "k8s ha: controller needs --ha-node-id=$(POD_INDEX), --ha-token and 3 --ha-peers" unless a.include?("--ha-node-id=$(POD_INDEX)") && a.include?("--ha-token-file=/run/secrets/wayhouse/ha-token") && a.any? { |x| x.start_with?("--ha-peers=") && x.split(",").size == 3 }
end
errs << "k8s single/web-gateway must keep the one-replica controller Deployment" unless %w[single web-gateway].all? { |o| k8s[o].any? { |d| d["kind"] == "Deployment" && d.dig("metadata", "name") == "wayhouse-controller" } }

# k8s is https between the Services (the wayhouse-tls Secret that `wayhouse-controller init --target k8s-secret`
# makes): controller and aggregator serve it, every client trusts the CA, and the plain-http opt-out
# survives only on the aggregator, which presents the instance token to node admin URLs that are http.
k8s.each do |o, docs|
  pods = docs.select { |d| %w[Deployment StatefulSet DaemonSet].include?(d["kind"]) }
  pods.each do |d|
    pod = d.dig("spec", "template", "spec")
    (pod["containers"] || []).each do |c|
      who = "k8s #{o}: #{d.dig("metadata", "name")}/#{c["name"]}"
      args = c["args"] || []
      args.grep(/\A--allow-insecure-/).each { |a| errs << "#{who} sets #{a}; only the aggregator may (node admin URLs are http)" unless c["name"] == "aggregator" && a == "--allow-insecure-controller" }
      args.grep(/\A--(controller|controller-url|aggregator|aggregator-url)=http:/).each { |a| errs << "#{who} reaches a Service over plain http (#{a})" }
      args.grep(/\A--ha-peers=/).each { |a| a.sub("--ha-peers=", "").split(",").each { |e| errs << "#{who} has a plain-http HA peer (#{e})" unless e =~ /\A\d+=https:\/\// } }
      mounts = (pod["volumes"] || []).select { |v| v.dig("secret", "secretName") == "wayhouse-tls" }
      errs << "#{who} must mount the wayhouse-tls Secret (CA at least)" if %w[controller aggregator ui wayhouse].include?(c["name"]) && mounts.empty?
      if %w[controller aggregator].include?(c["name"])
        errs << "#{who} must serve https (--tls-cert and --tls-key under /run/tls)" unless args.any? { |a| a =~ /\A--tls-cert=\/run\/tls\// } && args.any? { |a| a =~ /\A--tls-key=\/run\/tls\// }
        %w[readinessProbe livenessProbe].each { |k| errs << "#{who} #{k} must use scheme HTTPS" unless c.dig(k, "httpGet", "scheme") == "HTTPS" }
      elsif c["name"] != "web-sync" && c["name"] != "caddy"
        errs << "#{who} must trust the init CA (--ca-file=/run/tls/ca.crt)" unless args.include?("--ca-file=/run/tls/ca.crt")
      end
    end
  end
end
errs << "k8s ha: replicas must trust the CA for the Raft peers (--ca-file) and name https peers" unless sts && sts["spec"]["template"]["spec"]["containers"][0]["args"].include?("--ca-file=/run/tls/ca.crt")
# No key may sit in a manifest: the Secret is made by init, never rendered here.
k8s.each { |o, docs| docs.each { |d| errs << "k8s #{o}: a manifest must not define the wayhouse-tls Secret" if d["kind"] == "Secret" && d.dig("metadata", "name") == "wayhouse-tls" } }

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
errs << "tunnel override: controller lost the base flags" unless %w[--listen=0.0.0.0:9901 --data-dir=/data].all? { |f| ctl_cmd.include?(f) } && ctl_cmd.any? { |a| a.start_with?("--auth-token-file=") }
{ "wayhouse" => %w[DAC_OVERRIDE NET_ADMIN NET_BIND_SERVICE], "agent" => %w[DAC_OVERRIDE NET_ADMIN] }.each { |s, caps| errs << "tunnel #{s}: must drop all capabilities except #{caps.join("/")} and set no-new-privileges and read_only" unless tunnel[s]["cap_drop"] == ["ALL"] && (tunnel[s]["cap_add"] || []).sort == caps && tunnel[s]["read_only"] == true && tunnel[s]["security_opt"] == ["no-new-privileges:true"] }
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

# Solo bundle: plugins and a sealed secret store on the controller, a keyring that is created once
# and mounted read-only, and nothing published beyond the host loopback.
sc = solo["controller"]["command"]
errs << "solo: controller needs --plugins and --secret-key-file=/keys/secret.keys" unless sc.include?("--plugins") && sc.include?("--secret-key-file=/keys/secret.keys")
# https from the first start (`wayhouse-controller init` makes the certificates): the controller and
# the aggregator serve TLS, every client trusts the init CA, and no flag switches a guard off.
errs << "solo: the controller must serve https from the init certificates (--tls-cert/--tls-key under /tls)" unless sc.include?("--tls-cert=/tls/controller.pem") && sc.include?("--tls-key=/tls/controller.key")
errs << "solo: the aggregator must serve https from the init certificates" unless solo["aggregator"]["command"].include?("--tls-cert=/tls/aggregator.pem") && solo["aggregator"]["command"].include?("--tls-key=/tls/aggregator.key")
solo.each { |n, x| (x["command"] || []).each { |a| errs << "solo: #{n} switches off a plain-http guard (#{a}); the bundle is https" if a =~ /\A--allow-insecure-(controller|secrets)/ } }
%w[ui wayhouse].each do |svc|
  cmd = solo[svc]["command"]
  errs << "solo: #{svc} must trust the init CA (--ca-file=/tls/ca.pem) and mount solo-tls" unless cmd.include?("--ca-file=/tls/ca.pem") && solo[svc]["volumes"].any? { |v| v["target"] == "/tls" && v["source"] == "solo-tls" }
  errs << "solo: #{svc} reaches the controller or aggregator over plain http: #{cmd.grep(/\A--(controller|aggregator)(-url)?=http:/)}" unless cmd.grep(/\A--(controller|aggregator)(-url)?=http:/).empty?
end
errs << "solo: the seed step must POST over https with the init CA" unless solo["seed"]["command"].include?("https://controller:9901/config") && solo["seed"]["command"].include?("/tls/ca.pem")
errs << "solo: the keys step must install the certificates with the key files 0600" unless solo["keys"]["command"].join(" ").include?("chmod 0600 /tls/*.key")
errs << "solo: controller must wait for the keys step" unless solo["controller"]["depends_on"].dig("keys", "condition") == "service_completed_successfully"
errs << "solo: the keyring must be mounted read-only into the controller" unless solo["controller"]["volumes"].any? { |v| v["target"] == "/keys" && v["read_only"] == true }
sw = solo["wayhouse"]
errs << "solo: the proxy needs --secret-key-file=/keys/secret.keys, or a { secret: name } config stops it at start" unless sw["command"].include?("--secret-key-file=/keys/secret.keys")
# (The base file URLs are plain http; those are the ones the solo overlay swaps for https.)
plain_url = ->(a) { a =~ /\A--(controller|controller-url|aggregator|aggregator-url)=http:/ || a == "--allow-insecure-controller" }
errs << "solo: the proxy must keep every base flag" unless (base["wayhouse"]["command"].reject(&plain_url) - sw["command"]).empty?
errs << "solo: the keyring must be mounted read-only into the proxy, which must wait for the keys step" unless sw["volumes"].any? { |v| v["target"] == "/keys" && v["read_only"] == true } && sw["depends_on"].dig("keys", "condition") == "service_completed_successfully"
errs << "solo: keys step must never overwrite an existing keyring, and writes it atomically" unless solo["keys"]["command"].join(" ").include?("[ -s /keys/secret.keys ] ||") && solo["keys"]["command"].join(" ").include?("mv /keys/secret.keys.tmp /keys/secret.keys")
errs << "solo: ports must stay on the host loopback" unless solo.values.flat_map { |x| x["ports"] || [] }.all? { |pt| pt["host_ip"] == "127.0.0.1" }
errs << "solo: tokens must be required (${VAR:?}), not defaulted" if sc.grep(/\A--auth-token-file=/).empty?
errs << "solo-acme: must keep every solo flag and add --acme with a staging default" unless (sc - solo_acme["controller"]["command"]).empty? && solo_acme["controller"]["command"].include?("--acme") && solo_acme["controller"]["command"].include?("--acme-ca=letsencrypt-staging") && solo_acme["controller"]["command"].include?("--acme-accept-tos")
# Process flags (G1): every program shares one settings volume, and the base flags stay.
%w[controller aggregator ui wayhouse].each do |svc|
  errs << "solo: #{svc} needs --settings-dir=/settings and the solo-settings volume" unless solo[svc]["command"].include?("--settings-dir=/settings") && solo[svc]["volumes"].any? { |v| v["target"] == "/settings" && v["source"] == "solo-settings" }
  errs << "solo: #{svc} must keep every base flag" unless (base[svc]["command"].reject(&plain_url) - solo[svc]["command"]).empty?
end
# `init` writes private keys under tls/ next to the compose files: git must never see them.
errs << "tls/ is not git-ignored: wayhouse-controller init writes ca.key and the TLS keys to deploy/compose/tls/" unless system("git", "check-ignore", "-q", "deploy/compose/tls/ca.key")
# systemd units: tokens arrive as credentials (LoadCredential=), never as a flag value.
Dir["deploy/systemd/*.service"].each do |f|
  t = File.read(f)
  errs << "#{f}: puts a token or password on the command line" if t =~ /--[a-z-]*(token|password)(=|\s)/
  errs << "#{f}: must not read tokens from an EnvironmentFile" if t.include?("EnvironmentFile=") && t =~ /TOKEN|PASSWORD/
  errs << "#{f}: NoNewPrivileges missing" unless t.include?("NoNewPrivileges=yes")
end

# No compose or k8s command carries a token value: only `--<name>-file` flags.
[base, tunnel, web, solo, solo_acme].each do |cfg|
  cfg.each_value { |x| (x["command"] || []).each { |a| errs << "compose: #{a} puts a secret on the command line" if a =~ /\A--[a-z-]*(token|password)(=|\z)/ } }
end
# k8s args and commands too: only the -file flags may name a token or password.
k8s.each do |o, docs|
  docs.each do |d|
    pod = d.dig("spec", "template", "spec") or next
    (pod["containers"] || []).each do |c|
      ((c["args"] || []) + (c["command"] || [])).each { |a| errs << "k8s #{o}: #{c["name"]} puts a secret on its command line: #{a}" if a =~ /\A--[a-z-]*(token|password)(=|\z)/ }
    end
  end
end
if errs.empty? then puts "deploy lint: ok" else warn errs.map { |e| "LINT FAIL: #{e}" }; exit 1 end
'
