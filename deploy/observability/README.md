# deploy/observability/

A reference Grafana dashboard and Prometheus alert rules for wayhouse. Like the rest of
`deploy/` they are a starting point, not a supported deployment. Metric names and labels are
catalogued in [`docs/06`](../../docs/06-operations-observability.md).

| File                     | Purpose                                                                               |
| ------------------------ | ------------------------------------------------------------------------------------- |
| `grafana-dashboard.json` | import via Grafana → Dashboards → Import; pick your Prometheus datasource when asked  |
| `alerts.yml`             | add under `rule_files:` in `prometheus.yml`; two severities, `page` and `ticket`      |

Scrape every wayhouse instance's `/metrics`, and the controller's `/metrics` for the ACME
series. The dashboard and rules work with a single instance; the config-skew and gossip
alerts simply never fire there.

`make deploy-lint` fails if a rule or panel names a metric that does not exist (see
`deploy/check-observability.sh`), and runs `promtool check rules` when `promtool` is on `PATH`.

Wayhouse's histograms are exported as Prometheus summaries (`{quantile="0.95"}`, plus `_sum`
and `_count`), not as `_bucket` series, so use quantile selectors rather than
`histogram_quantile`.

## Runbooks

Each alert links here. Format: what it means, the first thing to check.

### WayhousePoolNoHealthyBackends
A pool has no healthy backend; its listeners cannot route. Check `wayhouse_healthcheck_total{result="fail"}` and `wayhouse_backend_connect_errors_total` for the pool, then the backends themselves.

### WayhouseBackendDomainDown
The gossip quorum marks a backend down although this instance sees it healthy. Check the other instances' view of that backend (network path between that instance and the backend).

### WayhouseFdUsageHigh
Open descriptors are above 80% of the soft limit. Check `wayhouse_active_connections`; if load is legitimate, raise `LimitNOFILE` / the container ulimit.

### WayhouseAcceptErrors
`accept` is failing, typically `EMFILE`. Check `wayhouse_fd_open / wayhouse_fd_limit` and raise the limit.

### WayhouseConfigVersionSkew
Instances run different config versions (the gauge is the unix time the config was applied). Expected briefly during a rollout; if it persists, find the lagging instance (`min(wayhouse_config_version)`) and check its reload errors and the controller push.

### WayhouseConfigReloadFailing
A reload failed and the previous config is still running. Read the instance log for the validation error.

### WayhouseListenerBindFailures
A listener could not (re)bind and the previous one, if any, keeps running. Check for a port in use or missing capability.

### WayhouseAcmeCertExpiryCritical
A certificate expires within two days (or already has). Check the site's status on the controller and `wayhouse_acme_issue_total{result="error"}`; see [`docs/acme.md`](../../docs/acme.md).

### WayhouseAcmeCertExpirySoon
A certificate expires within ten days although renewal starts at a third of its lifetime. Same checks as above. The series exist only on the Raft leader (or a standalone controller); the 30 minute `for` keeps a leader change from paging.

### WayhouseAcmeIssueErrors
The CA or DNS challenge failed in the last hour. Check the controller log and the site status; a CA rate limit pauses the site on its own.

### WayhouseAcmeKeyHeld
A node has no usable secret key for ACME. Check the cluster keyring on that node.

### WayhouseGossipPartition
An instance sees fewer gossip members than the best-connected instance. Check UDP reachability of the gossip port between instances and the shared gossip PSK.

### WayhouseWebSyncErrors
Caddy refused a document or a sync cycle failed before Caddy got it; the previous configuration keeps serving. Check the `web-sync` log and `wayhouse_web_sync_errors_total` by `stage`.

### WayhouseWebSyncStale
No successful web sync for ten minutes. Check that `web-sync` runs and can reach Caddy's admin socket.
