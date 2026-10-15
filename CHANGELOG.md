# Changelog

## [0.2.0](https://github.com/wayhouse-proxy/wayhouse/compare/v0.1.0...v0.2.0) (2026-10-15)


### ⚠ BREAKING CHANGES

* move the bundled sniffers to wayhouse-proxy/sniffers ([#245](https://github.com/wayhouse-proxy/wayhouse/issues/245))

### Features

* **plugin:** bound module compilation and add a conformance check ([#267](https://github.com/wayhouse-proxy/wayhouse/issues/267)) ([2d8744e](https://github.com/wayhouse-proxy/wayhouse/commit/2d8744ebb0f4419ee5633279f4e667aea40ff6a9))
* **plugin:** module replication, secrets with http, routes, webhooks and events ([#287](https://github.com/wayhouse-proxy/wayhouse/issues/287)) ([4cded9c](https://github.com/wayhouse-proxy/wayhouse/commit/4cded9ca532ebcfb9a1bb2172b7fae09ee8792f1))
* **plugin:** plugin ABI crate and host runtime with on_timer, log and state ([#264](https://github.com/wayhouse-proxy/wayhouse/issues/264)) ([f568aba](https://github.com/wayhouse-proxy/wayhouse/commit/f568aba039ba96aab1bd88fdbb29e71a27809d46))
* **plugin:** plugin install store and admin API on the standalone controller ([#269](https://github.com/wayhouse-proxy/wayhouse/issues/269)) ([1eace8e](https://github.com/wayhouse-proxy/wayhouse/commit/1eace8eeb23cd5cb526dd867d1237db860e6c304))
* **plugin:** replicate plugin installs and state through the HA state machine ([#281](https://github.com/wayhouse-proxy/wayhouse/issues/281)) ([7ca15b1](https://github.com/wayhouse-proxy/wayhouse/commit/7ca15b10c4ed44bac2e86edfabe823ad85916fad))
* **plugin:** serve /plugins on HA controllers and tick on the Raft leader ([#283](https://github.com/wayhouse-proxy/wayhouse/issues/283)) ([238fb31](https://github.com/wayhouse-proxy/wayhouse/commit/238fb31e3be97e5c58caec30bd30e6644464cb7b))
* **plugin:** tick enabled plugins on the standalone controller ([#272](https://github.com/wayhouse-proxy/wayhouse/issues/272)) ([8e65215](https://github.com/wayhouse-proxy/wayhouse/commit/8e65215907647d539733bf34838507b8272d8251))
* rolling fleet upgrades with a version-skew view and an upgrading guide ([#261](https://github.com/wayhouse-proxy/wayhouse/issues/261)) ([9aed0e5](https://github.com/wayhouse-proxy/wayhouse/commit/9aed0e52f6da5d20e3c382c7c619485e290e5056))
* sniffer upload keeps the previous module, rollback and automatic fallback ([#184](https://github.com/wayhouse-proxy/wayhouse/issues/184)) ([#244](https://github.com/wayhouse-proxy/wayhouse/issues/244)) ([f4a2a8c](https://github.com/wayhouse-proxy/wayhouse/commit/f4a2a8cff53317db68a4142d675e15c64d2693d2))
* **ui:** browse sniffer registries and install verified sniffers ([#229](https://github.com/wayhouse-proxy/wayhouse/issues/229)) ([8166edb](https://github.com/wayhouse-proxy/wayhouse/commit/8166edb75c58942f3834b055ee6ffa4fcd745745))
* **ui:** install sniffers from a registry on the Sniffers page ([#232](https://github.com/wayhouse-proxy/wayhouse/issues/232)) ([434f709](https://github.com/wayhouse-proxy/wayhouse/commit/434f709dbd2b5fe1dd353b34492729d833d09726))
* **ui:** plugins page to upload, approve, enable and delete plugins ([#275](https://github.com/wayhouse-proxy/wayhouse/issues/275)) ([ab1d7f3](https://github.com/wayhouse-proxy/wayhouse/commit/ab1d7f3a4839e59f312b6f06b7ce6f30a5168b09))
* update and roll back sniffers fleet-wide from the Sniffers page ([#184](https://github.com/wayhouse-proxy/wayhouse/issues/184)) ([#250](https://github.com/wayhouse-proxy/wayhouse/issues/250)) ([ad79529](https://github.com/wayhouse-proxy/wayhouse/commit/ad795293942a7ea114a404d69333f6b20567999a))
* wayhouse-registry crate with index format, verification and generator ([#224](https://github.com/wayhouse-proxy/wayhouse/issues/224)) ([bc41ee9](https://github.com/wayhouse-proxy/wayhouse/commit/bc41ee94ab597f8bec50e77e34491c83bdc64cab))


### Bug Fixes

* a replaced UDP group leaves the SO_REUSEPORT hash while it drains ([#256](https://github.com/wayhouse-proxy/wayhouse/issues/256)) ([9f905d5](https://github.com/wayhouse-proxy/wayhouse/commit/9f905d52fbea46326441b22507289fe6476543c8))
* accepted triage items (registry rules, relay warnings, peer retry, docs) ([#335](https://github.com/wayhouse-proxy/wayhouse/issues/335)) ([376c7ab](https://github.com/wayhouse-proxy/wayhouse/commit/376c7ab84963a89c3f3269963fa6b5fab5a12c29))
* batch of wave 3 follow-ups (sniffer swap cleanup, updates UI, CI timeouts, docs) ([#255](https://github.com/wayhouse-proxy/wayhouse/issues/255)) ([0750216](https://github.com/wayhouse-proxy/wayhouse/commit/075021642bed5eaa72bef58de45e581603b5383f))
* **core:** canonicalise IPv4-mapped peers and bound the rate limiter ([#314](https://github.com/wayhouse-proxy/wayhouse/issues/314)) ([020f746](https://github.com/wayhouse-proxy/wayhouse/commit/020f746f0fff2c42f1c682ca6def260d4d1c325f))
* **core:** fail the sniffer scan when a pinned module fails validation ([#212](https://github.com/wayhouse-proxy/wayhouse/issues/212)) ([426f726](https://github.com/wayhouse-proxy/wayhouse/commit/426f7260b7da67cf5b6366762d890a201ccc600e))
* **core:** gossip health versions survive a restart, add gossip frame magic byte ([#273](https://github.com/wayhouse-proxy/wayhouse/issues/273)) ([4c6c600](https://github.com/wayhouse-proxy/wayhouse/commit/4c6c600a8cb4b992001923f30841b3d4f05f3c48))
* enforce sniffer min_proxy and tidy UDP hand-off and listener reload ([#276](https://github.com/wayhouse-proxy/wayhouse/issues/276)) ([14218b9](https://github.com/wayhouse-proxy/wayhouse/commit/14218b9bf55bcf4894242e8df02cee69ece37f20))
* **ha:** harden forwarded writes, gossip registers, adopt and tunnel registries ([#325](https://github.com/wayhouse-proxy/wayhouse/issues/325)) ([8a5f680](https://github.com/wayhouse-proxy/wayhouse/commit/8a5f68047042543c25ac046ea56ced61ede72309))
* harden HA, gossip and tunnel registries before 0.2.0 ([#318](https://github.com/wayhouse-proxy/wayhouse/issues/318)) ([526c4e8](https://github.com/wayhouse-proxy/wayhouse/commit/526c4e86c18fea58ca2872f9e6d1c8f8679a9ee1))
* **ha:** schema gate and subscribe filter, secret revocation, upload cap exit, key-reuse log ([#345](https://github.com/wayhouse-proxy/wayhouse/issues/345)) ([34ddd5f](https://github.com/wayhouse-proxy/wayhouse/commit/34ddd5f87a6e6721ab884fc47aee3460a3779427))
* **ha:** serialize adoption with direct writes, drop non-member gossip registers ([#327](https://github.com/wayhouse-proxy/wayhouse/issues/327)) ([84b3d00](https://github.com/wayhouse-proxy/wayhouse/commit/84b3d004ceed0993fb0cd04dcf4e5359282cdebc))
* let fleet and UI sniffer uploads carry modules up to the proxy cap ([#228](https://github.com/wayhouse-proxy/wayhouse/issues/228)) ([a8c954c](https://github.com/wayhouse-proxy/wayhouse/commit/a8c954c5fb2ca99b08657433cce0cf21d572a86f))
* **plugins:** bound tick interval, cache webhook-only plugins, batch low-severity hardening ([#316](https://github.com/wayhouse-proxy/wayhouse/issues/316)) ([94754bf](https://github.com/wayhouse-proxy/wayhouse/commit/94754bf29c860d3424c513a3779d6682177e4fa8))
* **plugins:** harden the plugin host, store, HA gates and secret binding ([#328](https://github.com/wayhouse-proxy/wayhouse/issues/328)) ([a916dc3](https://github.com/wayhouse-proxy/wayhouse/commit/a916dc3d0a00e7e27d56ae790a33ea59d6ecf6c4))
* **proxy:** retry a failed config re-fetch after an overlay change ([#333](https://github.com/wayhouse-proxy/wayhouse/issues/333)) ([905d2cd](https://github.com/wayhouse-proxy/wayhouse/commit/905d2cd4a0ccb3baf1b77213bd0bfd89890dec91))
* small hardening batch (registry address guard, accept backoff, promote gate) ([#233](https://github.com/wayhouse-proxy/wayhouse/issues/233)) ([794cf27](https://github.com/wayhouse-proxy/wayhouse/commit/794cf27777bf3c3e1bc8d0c09d217d4e9c2d6e8c))
* **test:** hand out each fleet-test port once (ha_tls flake) ([#350](https://github.com/wayhouse-proxy/wayhouse/issues/350)) ([597ff53](https://github.com/wayhouse-proxy/wayhouse/commit/597ff533eb41c4db04939f5722c2f7b8bf7605c6))
* **test:** robust fleet startup wait and sniffer ticker test under load ([#332](https://github.com/wayhouse-proxy/wayhouse/issues/332)) ([5e77c7c](https://github.com/wayhouse-proxy/wayhouse/commit/5e77c7c3654e344c5e78d3357e84b2921653f868))
* tighten validators before 0.2.0 (plugin bounds, registry limits and URLs, discovery token URLs) ([#349](https://github.com/wayhouse-proxy/wayhouse/issues/349)) ([7e5ee1d](https://github.com/wayhouse-proxy/wayhouse/commit/7e5ee1d39050bf4328c42c1da1d80b40a1a13708))
* UI fonts, proxy Origin check, shared IPv6 allowlist, reload scans sniffers first ([#324](https://github.com/wayhouse-proxy/wayhouse/issues/324)) ([1f1aab6](https://github.com/wayhouse-proxy/wayhouse/commit/1f1aab6add97ced5b1b180dcee541179c9cabd53))
* **ui:** add cross-site guard, encode aggregator paths, bound sniffer stores, batch lows ([#323](https://github.com/wayhouse-proxy/wayhouse/issues/323)) ([178ce88](https://github.com/wayhouse-proxy/wayhouse/commit/178ce8811aaa351c0dcd1a42a49fc2944334df18))
* uniform login throttle, UI/core lows, registry and relay follow-ups ([#346](https://github.com/wayhouse-proxy/wayhouse/issues/346)) ([7ee1719](https://github.com/wayhouse-proxy/wayhouse/commit/7ee1719cc46a344f26c16e0ba8355ebcc74603a7))


### Code Refactoring

* move the bundled sniffers to wayhouse-proxy/sniffers ([#245](https://github.com/wayhouse-proxy/wayhouse/issues/245)) ([2559981](https://github.com/wayhouse-proxy/wayhouse/commit/2559981f6529ed2b5ca3768b06be032b261df8ab))

## [0.1.0](https://github.com/wayhouse-proxy/wayhouse/compare/v0.0.1...v0.1.0) (2026-10-07)


### Features

* plugin ABI version declared in the module and enforced by the host ([#199](https://github.com/wayhouse-proxy/wayhouse/issues/199)) ([8c2b449](https://github.com/wayhouse-proxy/wayhouse/commit/8c2b4499f5dd4990d186386b570b8b7be04a5877))
* protocol, config and store version fields ([#200](https://github.com/wayhouse-proxy/wayhouse/issues/200)) ([397c5da](https://github.com/wayhouse-proxy/wayhouse/commit/397c5da3a320174f3e92504254ed3304caf93efd)), closes [#192](https://github.com/wayhouse-proxy/wayhouse/issues/192)


### Bug Fixes

* **ci:** leave the generated CHANGELOG.md out of the Prettier check ([#188](https://github.com/wayhouse-proxy/wayhouse/issues/188)) ([#206](https://github.com/wayhouse-proxy/wayhouse/issues/206)) ([1728475](https://github.com/wayhouse-proxy/wayhouse/commit/172847519348cb06d7c99e59e98b9ae584670404))
* **ci:** use the real release-please branch name ([#188](https://github.com/wayhouse-proxy/wayhouse/issues/188)) ([#204](https://github.com/wayhouse-proxy/wayhouse/issues/204)) ([236d87c](https://github.com/wayhouse-proxy/wayhouse/commit/236d87cffff048d207187dae6c87022cd93a9746))
