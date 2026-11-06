# systemd units for a solo box

The same bundle as `deploy/compose/compose.solo.yml`, without containers: controller (plugins and
a sealed secret store), aggregator, UI and the proxy, all on loopback. Reference only, like the
rest of `deploy/`; [`docs/12-deployment.md`](../../docs/12-deployment.md) ("One box") has the
walk-through.

```sh
# binaries (from a release or `cargo build --release`) in /usr/local/bin:
#   wayhouse wayhouse-controller wayhouse-aggregator wayhouse-ui
sudo install -m 0644 deploy/systemd/wayhouse.sysusers /etc/sysusers.d/wayhouse.conf
sudo systemd-sysusers
sudo install -d -m 0755 /etc/wayhouse
sudo sh deploy/solo/init.sh /etc/wayhouse/solo.env        # random tokens, mode 0600, nothing printed
sudo install -m 0644 deploy/systemd/*.service deploy/systemd/*.target /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now wayhouse-solo.target
```

The UI is at <http://127.0.0.1:9903>; the password is `WAYHOUSE_UI_PASSWORD` in
`/etc/wayhouse/solo.env`. The first proxy config goes in through the UI (Settings) or
`curl --data-binary @wayhouse.yaml -H "Authorization: Bearer $TOKEN" http://127.0.0.1:9901/config`.

`wayhouse-keygen.service` creates `/var/lib/wayhouse/secret.keys` once and never replaces it. **Back it
up with the controller's data**: without it every stored secret is unreadable.

Caveat: the binaries take their tokens as command-line flags, so any local user can read them in
`ps`. Run this on a box where that is acceptable, or mount `/proc` with `hidepid=2`.
