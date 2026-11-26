---
title: Set up a single box
description: "The basic setup most users run: proxy, controller, UI and sealed secrets on one machine."
sidebar:
    order: 3
---

The solo bundle is the setup most people start with: one machine runs the proxy and the
control plane (controller, aggregator and UI) together. Every feature works on one box;
nothing here needs a second node.

## Start it

```sh
cd deploy/compose
wayhouse-controller init --target compose   # once: tokens, CA and certificates
docker compose -f docker-compose.yml -f compose.solo.yml up -d --build
```

`init` writes the tokens to `.env` (mode `0600`) and a private CA with the certificates to
`tls/`. It prints no secret and never overwrites existing files, so it is safe to run again.
Without the binary on your `PATH`, `sh ../solo/init.sh` does the same.

Then open <http://127.0.0.1:9903>. The password is `WAYHOUSE_UI_PASSWORD` in `.env`.

What you get:

- **https between the services from the first start.** The controller and aggregator serve
  TLS with certificates from the private CA; the UI and the proxy trust only that CA. No
  `--allow-insecure-*` flag is needed. The certificates last five years, the CA ten.
- **Tokens as secret files**, never on a command line: Compose mounts each token under
  `/run/secrets`.
- **A sealed secret store.** A one-shot step creates the keyring on the first start and never
  replaces it. Plugin secrets and `{ secret: name }` references in the config work, and you
  manage them in the UI under Settings, Secrets.
- **Loopback only.** Every published port is bound to `127.0.0.1`.

> [!WARNING]
> Back up the `solo-keys` volume together with the controller's data. Without the key every
> stored secret is unreadable.

## Load your first config

The proxy's first config goes in through the UI (Settings) or straight to the controller:

```sh
curl --cacert tls/ca.pem --data-binary @wayhouse.yaml \
  -H "Authorization: Bearer $TOKEN" https://127.0.0.1:9901/config
```

Start from `deploy/config.example.yaml` and read [Your first listener and
pool](first-listener-and-pool.md). The controller validates a config before it accepts it, and
proxies pick the new version up on their own.

## Reach it from another machine

The certificates are valid for `localhost`, `127.0.0.1`, `::1` and the service name
`controller`. To reach the controller by another name, add it **before the first start**:

```sh
wayhouse-controller init --target compose --san controller.example.org
```

The UI stays on plain http on the loopback address. To expose it, put a TLS front door in front
of it: the `compose.web.yml` overlay adds Caddy for that, see the
[web proxy guide](../../web-proxy.md).

## Optional pieces

| Add                        | What for                                                                                                 |
| -------------------------- | -------------------------------------------------------------------------------------------------------- |
| `compose.solo-control.yml` | Run controller, aggregator and UI as one `wayhouse-control` process (`init --target compose --control`). |
| `compose.web.yml`          | A Caddy front door for websites, see [Web proxy](../../web-proxy.md).                                    |
| `compose.solo-acme.yml`    | Certificates from an ACME CA, see [Certificates](../../acme.md).                                         |

## Without containers

`sudo wayhouse-controller init --target systemd` writes `/etc/wayhouse/credentials` and
`/etc/wayhouse/tls`; then `systemctl enable --now wayhouse-solo.target`. See
[Install](install.md).

## Where next

- [Your first listener and pool](first-listener-and-pool.md)
- [Route players by rules](../routing/route-by-rules.md)
- [Monitor a proxy](../operate/monitor.md)
