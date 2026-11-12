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
sudo sh deploy/solo/init.sh --credentials /etc/wayhouse/credentials   # random tokens, mode 0600, nothing printed
sudo install -m 0644 deploy/systemd/*.service deploy/systemd/*.target /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now wayhouse-solo.target
```

The UI is at <http://127.0.0.1:9903>; the password is in
`/etc/wayhouse/credentials/ui-password`. The first proxy config goes in through the UI (Settings) or
`curl --data-binary @wayhouse.yaml -H "Authorization: Bearer $TOKEN" http://127.0.0.1:9901/config`.

`wayhouse-keygen.service` creates `/var/lib/wayhouse/secret.keys` once and never replaces it. **Back it
up with the controller's data**: without it every stored secret is unreadable.

The units pass each token to its program as a systemd credential (`LoadCredential=`): the program
reads `$CREDENTIALS_DIRECTORY/<name>`, so no token is ever on a command line or in the environment.
