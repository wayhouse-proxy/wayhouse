#!/bin/sh
# deploy/observability/ must only name metrics that exist (RF-6): a renamed metric would
# otherwise leave a silent alert or an empty panel. Known names = the two metrics_defs.rs
# files, the ACME and TLS names registered elsewhere, and what docs/06 documents.
# Also: the rules and dashboard parse, and the dashboard keeps its ${DS_PROMETHEUS} datasource.
# promtool (Prometheus) is optional: used when on PATH, skipped with a message otherwise.
set -eu
cd "$(dirname "$0")/.."
ruby -Eutf-8 -rjson -ryaml -e '
known = Dir["crates/*/src/metrics_defs.rs"].flat_map { |f| File.read(f).scan(/"(wayhouse_[a-z0-9_]+)"/).flatten }
known += File.read("docs/06-operations-observability.md").scan(/`(wayhouse_[a-z0-9_]+)/).flatten
known += File.read("crates/wayhouse-controller/src/acme/runner.rs").scan(/(?:gauge|counter)!\("(wayhouse_[a-z0-9_]+)"/).flatten
abort "no known metrics found: check-observability.sh is out of date" if known.size < 30
exprs = []
rules = YAML.load_file("deploy/observability/alerts.yml")["groups"].flat_map { |g| g["rules"] }
exprs += rules.map { |r| r["expr"] }
dash = JSON.parse(File.read("deploy/observability/grafana-dashboard.json"))
walk = ->(n) { case n when Hash then (exprs << n["expr"] if n["expr"].is_a?(String)); n.each_value(&walk) when Array then n.each(&walk) end }
walk.(dash)
# summaries/histograms add _sum/_count/_bucket to the base name
bad = exprs.flat_map { |e| e.scan(/\bwayhouse_[a-z0-9_]+/) }.uniq.reject { |m| known.include?(m) || known.include?(m.sub(/_(sum|count|bucket)\z/, "")) }
errs = []
errs << "unknown metric names in deploy/observability: #{bad.join(", ")}" unless bad.empty?
errs << "dashboard lacks the DS_PROMETHEUS datasource variable" unless dash.dig("templating", "list").to_a.any? { |v| v["name"] == "DS_PROMETHEUS" && v["type"] == "datasource" }
errs << "alert rules without severity/runbook_url: #{rules.reject { |r| r.dig("labels", "severity") && r.dig("annotations", "runbook_url") }.map { |r| r["alert"] }.join(", ")}" unless rules.all? { |r| r.dig("labels", "severity") && r.dig("annotations", "runbook_url") }
if errs.empty? then puts "observability lint: ok (#{rules.size} rules, #{exprs.size - rules.size} panel queries)" else warn errs.map { |e| "LINT FAIL: #{e}" }; exit 1 end
'
if command -v promtool >/dev/null 2>&1; then
  promtool check rules deploy/observability/alerts.yml
else
  echo "promtool not found: skipped 'promtool check rules' (install Prometheus to run it)"
fi
