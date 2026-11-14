# Changelog

## [0.4.0](https://github.com/wayhouse-proxy/wayhouse/compare/v0.3.0...v0.4.0) (2026-11-14)


### ⚠ BREAKING CHANGES

* **security:** tokens from files, not argv ([#628](https://github.com/wayhouse-proxy/wayhouse/issues/628))
* **plugins:** plugin ABI 0.2 with manifest options and capability bindings ([#486](https://github.com/wayhouse-proxy/wayhouse/issues/486))

### Features

* **aggregator:** AG1 fleet feature rollup, config revision, node list and stats fixes ([#554](https://github.com/wayhouse-proxy/wayhouse/issues/554)) ([3cc0a8f](https://github.com/wayhouse-proxy/wayhouse/commit/3cc0a8f6fae32a97d6a147fb54304f891e598caa))
* **aggregator:** paged, filtered node list with region and tags ([#502](https://github.com/wayhouse-proxy/wayhouse/issues/502)) ([ef00c32](https://github.com/wayhouse-proxy/wayhouse/commit/ef00c324a723c5df34738e777529e60e3ebef79f))
* **aggregator:** rolling fleet stats and GET /fleet/stats ([#492](https://github.com/wayhouse-proxy/wayhouse/issues/492)) ([11fdf87](https://github.com/wayhouse-proxy/wayhouse/commit/11fdf875862284093deaf0904e107e4baea01049))
* **config:** admin token and gossip PSK accept { secret: name } (K1b) ([#573](https://github.com/wayhouse-proxy/wayhouse/issues/573)) ([ac81df0](https://github.com/wayhouse-proxy/wayhouse/commit/ac81df0404db6582e61c07930be8d46dabd73bf8))
* **config:** feature-flag registry, features: section and GET /features ([#512](https://github.com/wayhouse-proxy/wayhouse/issues/512)) ([ed95b9b](https://github.com/wayhouse-proxy/wayhouse/commit/ed95b9bf48632fe52cfdd1352100097b1f2fc30c))
* **config:** per-item enabled: to park listeners, sources, resolvers, sites and filters (F2) ([#559](https://github.com/wayhouse-proxy/wayhouse/issues/559)) ([d21fef1](https://github.com/wayhouse-proxy/wayhouse/commit/d21fef139e391f31f9cace7ee7bdc08e6335bc52))
* **control:** compose overlay, image and smoke test for wayhouse-control ([#667](https://github.com/wayhouse-proxy/wayhouse/issues/667)) ([ea10e97](https://github.com/wayhouse-proxy/wayhouse/commit/ea10e97cc23df104e5a3615513c1aab0678ff4cb))
* **control:** init --control and systemd unit for wayhouse-control ([#664](https://github.com/wayhouse-proxy/wayhouse/issues/664)) ([fc92492](https://github.com/wayhouse-proxy/wayhouse/commit/fc92492004f9f9316654cf5cfaeb30c020906c32))
* **controller:** cluster secret store and proxy unseal (K1c) ([#574](https://github.com/wayhouse-proxy/wayhouse/issues/574)) ([5acf160](https://github.com/wayhouse-proxy/wayhouse/commit/5acf160e4ba3fef41939d103b5188b8c6aa5c31c))
* **controller:** config validate endpoint, base-revision guard and schema endpoint ([#485](https://github.com/wayhouse-proxy/wayhouse/issues/485)) ([469ad0d](https://github.com/wayhouse-proxy/wayhouse/commit/469ad0d8116e370d949b324a124e0a7217303bf2))
* **controller:** confirm-and-revert for risky config revisions ([#500](https://github.com/wayhouse-proxy/wayhouse/issues/500)) ([4d17d58](https://github.com/wayhouse-proxy/wayhouse/commit/4d17d585bd3c6edd285c24b9278510b135040896))
* **controller:** emergency feature-flag overrides ([#517](https://github.com/wayhouse-proxy/wayhouse/issues/517)) ([85bf370](https://github.com/wayhouse-proxy/wayhouse/commit/85bf370d7cce5319714b91e08b86a38e33c147b6))
* **controller:** reconcile plugin installs from a plugins file (P6) ([#571](https://github.com/wayhouse-proxy/wayhouse/issues/571)) ([067ef7a](https://github.com/wayhouse-proxy/wayhouse/commit/067ef7ada18284c18113f600312059d4ab5f7363))
* **controller:** separate proxy and agent tokens from the admin token ([#617](https://github.com/wayhouse-proxy/wayhouse/issues/617)) ([25b9153](https://github.com/wayhouse-proxy/wayhouse/commit/25b91533de9297f283c066da971811def931530e))
* **controller:** stored UI data (dashboards, themes, filters, layout, defaults) ([#497](https://github.com/wayhouse-proxy/wayhouse/issues/497)) ([2e42b4d](https://github.com/wayhouse-proxy/wayhouse/commit/2e42b4d36a4a14d88ac783432e74a9dd76f59ff9))
* **controller:** suspend and resume plugin capabilities (F5) ([#566](https://github.com/wayhouse-proxy/wayhouse/issues/566)) ([b26a6d6](https://github.com/wayhouse-proxy/wayhouse/commit/b26a6d6dd2d65aec95c715dcdcae7a69bc89cd95))
* **controller:** theme contrast checks on save ([#508](https://github.com/wayhouse-proxy/wayhouse/issues/508)) ([d7e4225](https://github.com/wayhouse-proxy/wayhouse/commit/d7e42259eb0421dc3863bd6482cc295bdfd39036))
* **controller:** wire the control-plane feature flags (F3b) ([#565](https://github.com/wayhouse-proxy/wayhouse/issues/565)) ([e228d15](https://github.com/wayhouse-proxy/wayhouse/commit/e228d154c188830df07c9c07b0765bbf5e6d7731))
* **control:** wayhouse-control, controller + aggregator + UI in one process ([#663](https://github.com/wayhouse-proxy/wayhouse/issues/663)) ([c9e3100](https://github.com/wayhouse-proxy/wayhouse/commit/c9e31007396f8092ad09fcc65bbf778f5d09364e))
* **deploy:** Grafana dashboard and Prometheus alert rules ([#451](https://github.com/wayhouse-proxy/wayhouse/issues/451)) ([aa0b9ef](https://github.com/wayhouse-proxy/wayhouse/commit/aa0b9ef4b024ff8f7a773f6807859ab982433f75))
* **deploy:** Kustomize overlays for Kubernetes (single, ha, web-gateway) ([#453](https://github.com/wayhouse-proxy/wayhouse/issues/453)) ([94dd5dc](https://github.com/wayhouse-proxy/wayhouse/commit/94dd5dc076f0fc4535e4c7d7d060385108300599))
* **deploy:** solo bundle for a single box (S1) ([#578](https://github.com/wayhouse-proxy/wayhouse/issues/578)) ([8700f41](https://github.com/wayhouse-proxy/wayhouse/commit/8700f41b9f87859760577d8ddc422dca11fcea9a))
* **enroll:** --ha-node-id-file for an enrolled controller replica ([#660](https://github.com/wayhouse-proxy/wayhouse/issues/660)) ([b214aa7](https://github.com/wayhouse-proxy/wayhouse/commit/b214aa7531135a791733667da5c8376e6a0d375f))
* **enroll:** pool controller codes that allocate Raft node ids ([#659](https://github.com/wayhouse-proxy/wayhouse/issues/659)) ([8b0efcf](https://github.com/wayhouse-proxy/wayhouse/commit/8b0efcf9fac9318809e643f09fb32241891e0ffd))
* **fleet:** operator-edited node tags (PUT/DELETE /api/fleet/nodes/{id}/tags) ([#524](https://github.com/wayhouse-proxy/wayhouse/issues/524)) ([12f5367](https://github.com/wayhouse-proxy/wayhouse/commit/12f53670dd7dcd15101d743b48f7ec088fbac371))
* **ha:** join a controller replica by one-time code ([#643](https://github.com/wayhouse-proxy/wayhouse/issues/643)) ([f283434](https://github.com/wayhouse-proxy/wayhouse/commit/f283434e7e80a610efb6fb99a522082b93cfdd06))
* **k8s:** ha-enrolled overlay, controller replicas join with one pool code ([#661](https://github.com/wayhouse-proxy/wayhouse/issues/661)) ([e01eceb](https://github.com/wayhouse-proxy/wayhouse/commit/e01ecebbd8241daeea722a5ccc72767b58b03c76))
* **k8s:** per-node enrollment with one multi-use code ([#657](https://github.com/wayhouse-proxy/wayhouse/issues/657)) ([5494a5e](https://github.com/wayhouse-proxy/wayhouse/commit/5494a5e014aefbcb876c3f71e38b33908620d812))
* **plugins:** config edit API and UI proxies for secrets, webhook, status ([#506](https://github.com/wayhouse-proxy/wayhouse/issues/506)) ([248b5b9](https://github.com/wayhouse-proxy/wayhouse/commit/248b5b9278c500aa6ed15da304099b04a92f61fb))
* **plugins:** plugin ABI 0.2 with manifest options and capability bindings ([#486](https://github.com/wayhouse-proxy/wayhouse/issues/486)) ([10201de](https://github.com/wayhouse-proxy/wayhouse/commit/10201de6bdf0ae7a8ada5a1ca9e7a416fbc9766f))
* **proxy:** sealed secret store for file-config proxies (S2a) ([#579](https://github.com/wayhouse-proxy/wayhouse/issues/579)) ([26ca825](https://github.com/wayhouse-proxy/wayhouse/commit/26ca825fd9503b7e143ee32d026d232d79bb7cf3))
* **proxy:** wire the data-plane feature flags (F3a) ([#562](https://github.com/wayhouse-proxy/wayhouse/issues/562)) ([87cebb5](https://github.com/wayhouse-proxy/wayhouse/commit/87cebb558364930ec3104261554a6fcbc0e3de57))
* **registry:** index schema 2 with a catalog, detail files, categories and tags ([#651](https://github.com/wayhouse-proxy/wayhouse/issues/651)) ([44a0a24](https://github.com/wayhouse-proxy/wayhouse/commit/44a0a247797549097f482368ca642107db5763ea))
* **registry:** signatures must name the artifact they sign ([#612](https://github.com/wayhouse-proxy/wayhouse/issues/612)) ([5f84f28](https://github.com/wayhouse-proxy/wayhouse/commit/5f84f28759390d21d3b9d671f51b26c94c95284f))
* **registry:** upgrade a schema 1 previous index in wayhouse-registry-gen ([#655](https://github.com/wayhouse-proxy/wayhouse/issues/655)) ([952bc6e](https://github.com/wayhouse-proxy/wayhouse/commit/952bc6e555c12644aafbf2c2d2f242a2c8931cb1))
* **registry:** verify-plugin in wayhouse-registry-gen runs the client's plugin-index validator ([#698](https://github.com/wayhouse-proxy/wayhouse/issues/698)) ([066879b](https://github.com/wayhouse-proxy/wayhouse/commit/066879b1f89dbc149b20bab63e65f7308365d0b3))
* **security:** enroll proxies, aggregators and UIs by one-time code ([#636](https://github.com/wayhouse-proxy/wayhouse/issues/636)) ([ef8a794](https://github.com/wayhouse-proxy/wayhouse/commit/ef8a794d2b74593a02f91dbadd3276167161d3f9))
* **security:** hybrid post-quantum TLS key exchange (X25519MLKEM768) ([#632](https://github.com/wayhouse-proxy/wayhouse/issues/632)) ([c3324fe](https://github.com/wayhouse-proxy/wayhouse/commit/c3324fe85bf4fca5daa285850013cd4a48487d14))
* **security:** Kubernetes manifests on https with the init TLS Secret ([#640](https://github.com/wayhouse-proxy/wayhouse/issues/640)) ([0782538](https://github.com/wayhouse-proxy/wayhouse/commit/0782538dde79ea443330330302f9b606f27d127d))
* **security:** one-time-code enrollment for origin agents with per-origin tunnel PSK ([#633](https://github.com/wayhouse-proxy/wayhouse/issues/633)) ([2bdfd31](https://github.com/wayhouse-proxy/wayhouse/commit/2bdfd31f60b383443e83d752cb594712ae18e92c))
* **security:** rotate an origin's tunnel PSK and issue per-origin tokens ([#650](https://github.com/wayhouse-proxy/wayhouse/issues/650)) ([0ff5e5d](https://github.com/wayhouse-proxy/wayhouse/commit/0ff5e5d519a94b182bbb438b3484da7b10f6ca90))
* **security:** tokens from files, not argv ([#628](https://github.com/wayhouse-proxy/wayhouse/issues/628)) ([435eb26](https://github.com/wayhouse-proxy/wayhouse/commit/435eb26e04bb4603f45e06dbc632a5642bb1bcfc))
* **security:** wayhouse-controller init with TLS certificates; solo bundles on https ([#635](https://github.com/wayhouse-proxy/wayhouse/issues/635)) ([70e0b34](https://github.com/wayhouse-proxy/wayhouse/commit/70e0b344b0e9dc635bd26349109411989152ba7d))
* **settings:** process flags from a settings file with trial applies (G1a) ([#586](https://github.com/wayhouse-proxy/wayhouse/issues/586)) ([1f43843](https://github.com/wayhouse-proxy/wayhouse/commit/1f438437d02c96caecc0c838d94315e3960fffc0))
* **ui:** --trusted-proxy for a per-client login delay behind a proxy ([#704](https://github.com/wayhouse-proxy/wayhouse/issues/704)) ([bd5a67b](https://github.com/wayhouse-proxy/wayhouse/commit/bd5a67b81c729438733cc2e9a2d6f08c3833e969))
* **ui:** Add node mints a pool controller code for Kubernetes ([#666](https://github.com/wayhouse-proxy/wayhouse/issues/666)) ([e28fc3f](https://github.com/wayhouse-proxy/wayhouse/commit/e28fc3f720149525295159f169d24eb5fc884363))
* **ui:** Add node page generates the one-command setup for every role ([#645](https://github.com/wayhouse-proxy/wayhouse/issues/645)) ([a5b2901](https://github.com/wayhouse-proxy/wayhouse/commit/a5b2901bb24a6c8998fafcd5bea84c84b3563cfa))
* **ui:** config editing with confirm-and-revert, canary and source editor (A3b) ([#526](https://github.com/wayhouse-proxy/wayhouse/issues/526)) ([5d4ff63](https://github.com/wayhouse-proxy/wayhouse/commit/5d4ff639e2146ca5afbb2e80d1330c9fb52b7739))
* **ui:** controller replica role in Add node ([#656](https://github.com/wayhouse-proxy/wayhouse/issues/656)) ([5c9991f](https://github.com/wayhouse-proxy/wayhouse/commit/5c9991f5ae1c407bc0969a1ef50dd0ff0aac54e3))
* **ui:** dashboard page and widget system (W1) ([#514](https://github.com/wayhouse-proxy/wayhouse/issues/514)) ([1921af3](https://github.com/wayhouse-proxy/wayhouse/commit/1921af3b2fa85f720ade02f57eefff17091dcdfe))
* **ui:** design-system foundation (U1a) ([#490](https://github.com/wayhouse-proxy/wayhouse/issues/490)) ([37d375e](https://github.com/wayhouse-proxy/wayhouse/commit/37d375e5e25fcb943f66ba778c2ffca6fd44bb42))
* **ui:** edit a node's operator tags in the fleet inspector ([#531](https://github.com/wayhouse-proxy/wayhouse/issues/531)) ([d82546e](https://github.com/wayhouse-proxy/wayhouse/commit/d82546e1fb44e3b100994cef6a5d666015e00d4a))
* **ui:** edit process flags from the UI with trial applies (G1b) ([#587](https://github.com/wayhouse-proxy/wayhouse/issues/587)) ([d81b4af](https://github.com/wayhouse-proxy/wayhouse/commit/d81b4af2b2b49b5554fef8a99b9c5c5e17b59eaf))
* **ui:** features page with source badges and emergency overrides (F6) ([#540](https://github.com/wayhouse-proxy/wayhouse/issues/540)) ([3a7d7be](https://github.com/wayhouse-proxy/wayhouse/commit/3a7d7bed4695d0678c3fa6a14b0ac5f07777eb6f))
* **ui:** fetch plugins from a registry (P5) ([#568](https://github.com/wayhouse-proxy/wayhouse/issues/568)) ([565eaf1](https://github.com/wayhouse-proxy/wayhouse/commit/565eaf142380a147af4197a3547db2580c62eaf2))
* **ui:** fleet page for 500 nodes (FL1) ([#527](https://github.com/wayhouse-proxy/wayhouse/issues/527)) ([8b47b86](https://github.com/wayhouse-proxy/wayhouse/commit/8b47b861bcab3b0a28a064ca3ea76bbb449c372f))
* **ui:** install a plugin from a registry (P5-UI) ([#577](https://github.com/wayhouse-proxy/wayhouse/issues/577)) ([4c8952b](https://github.com/wayhouse-proxy/wayhouse/commit/4c8952b83bf4de05aab1300bdaff021baf136d7c))
* **ui:** interactive demo with an in-browser mock backend ([#547](https://github.com/wayhouse-proxy/wayhouse/issues/547)) ([712c07c](https://github.com/wayhouse-proxy/wayhouse/commit/712c07ce99997aa3276fbb5acd37258323a0cbe0))
* **ui:** light navigation rail in the light theme ([#545](https://github.com/wayhouse-proxy/wayhouse/issues/545)) ([3bc3c57](https://github.com/wayhouse-proxy/wayhouse/commit/3bc3c5734ba28c21dbe161fec7133177a9952b9c))
* **ui:** listeners and routes editor with patch-line diagram (A5a) ([#529](https://github.com/wayhouse-proxy/wayhouse/issues/529)) ([13f49a2](https://github.com/wayhouse-proxy/wayhouse/commit/13f49a21f12591cbdcb22107321ae726a3619e4f))
* **ui:** managed-by-file badge for plugin installs (P6-UI) ([#580](https://github.com/wayhouse-proxy/wayhouse/issues/580)) ([0999382](https://github.com/wayhouse-proxy/wayhouse/commit/0999382ff644c35191aad8e779ecb62dc1568121))
* **ui:** per-node feature view, plain flag names, inline clear, kill-switch runbook (F6b, F7) ([#572](https://github.com/wayhouse-proxy/wayhouse/issues/572)) ([20f40ec](https://github.com/wayhouse-proxy/wayhouse/commit/20f40ecd734a43881ab37dc5852ba8e2a46dcdb0))
* **ui:** plugin install wizard and install pages (P4) ([#518](https://github.com/wayhouse-proxy/wayhouse/issues/518)) ([424bd36](https://github.com/wayhouse-proxy/wayhouse/commit/424bd36dd7d7c7d334e256b0b8babed54990b9d0))
* **ui:** pools, backend sources and resolvers editors (A5b) ([#534](https://github.com/wayhouse-proxy/wayhouse/issues/534)) ([5e98cfb](https://github.com/wayhouse-proxy/wayhouse/commit/5e98cfbf79f7445a43dce666af9bff5dadcc54ec))
* **ui:** Rotate key button for an origin's tunnel key ([#654](https://github.com/wayhouse-proxy/wayhouse/issues/654)) ([26e7973](https://github.com/wayhouse-proxy/wayhouse/commit/26e79738bf5c358f1f39a42dbf4dd66452ca17ac))
* **ui:** schema-driven config form and comment-preserving YAML patch layer (A3a) ([#495](https://github.com/wayhouse-proxy/wayhouse/issues/495)) ([22f7148](https://github.com/wayhouse-proxy/wayhouse/commit/22f7148915e86f071df98fa4d9a606be57431648))
* **ui:** search, filter and sort the sniffer and plugin catalogs ([#652](https://github.com/wayhouse-proxy/wayhouse/issues/652)) ([859c5e6](https://github.com/wayhouse-proxy/wayhouse/commit/859c5e607bc2970ce99bf8bd88dd465bc9393a35))
* **ui:** secrets page and move-to-secret-store (K1-UI) ([#576](https://github.com/wayhouse-proxy/wayhouse/issues/576)) ([ae91514](https://github.com/wayhouse-proxy/wayhouse/commit/ae91514d33d09850b8e7e582911c77540e79c7d2))
* **ui:** set the official registry signing key (R1) ([#575](https://github.com/wayhouse-proxy/wayhouse/issues/575)) ([339e617](https://github.com/wayhouse-proxy/wayhouse/commit/339e61742fe867c7d7187c915f19ad94d336443e))
* **ui:** Settings &gt; Appearance and Layout (stored themes and form layout) ([#510](https://github.com/wayhouse-proxy/wayhouse/issues/510)) ([c0d1304](https://github.com/wayhouse-proxy/wayhouse/commit/c0d13045720b1b431b5f02d0ed29326590303984))
* **ui:** shell, session role and the existing pages on the design system (U1b) ([#494](https://github.com/wayhouse-proxy/wayhouse/issues/494)) ([89aa4d2](https://github.com/wayhouse-proxy/wayhouse/commit/89aa4d26e1be60ddbdd5b5cb273507c22a6a903a))
* **ui:** stats charts and Overview, Listeners and Pools pages (E2) ([#504](https://github.com/wayhouse-proxy/wayhouse/issues/504)) ([1a3422b](https://github.com/wayhouse-proxy/wayhouse/commit/1a3422bb97c06fb4892da4333fa95f4b418ba446))
* **ui:** web sites editor and ACME site writes (A5c) ([#538](https://github.com/wayhouse-proxy/wayhouse/issues/538)) ([4a3994b](https://github.com/wayhouse-proxy/wayhouse/commit/4a3994bdb585c509e645a5b9319371685521b0c8))
* **wayhouse:** config schema and validate on the proxy admin API (S2c) ([#585](https://github.com/wayhouse-proxy/wayhouse/issues/585)) ([8a69876](https://github.com/wayhouse-proxy/wayhouse/commit/8a698767ce78cbf5ae28fb20299a4f60b78915b1))
* **wayhouse:** plugin routes on a file config with --plugin-overlay-url (S2b) ([#584](https://github.com/wayhouse-proxy/wayhouse/issues/584)) ([2e24133](https://github.com/wayhouse-proxy/wayhouse/commit/2e2413393af24b21be29df2a35eeb7ef784e76b7))


### Bug Fixes

* **acme:** DNS-01 visibility via authoritative nameservers, cleanup retry, --ca-file ([#706](https://github.com/wayhouse-proxy/wayhouse/issues/706)) ([011ddbf](https://github.com/wayhouse-proxy/wayhouse/commit/011ddbf0010ed4ddc3c3382eca5e7305b399df79))
* **aggregator:** refuse loopback admin_url unless --instance-url-allow lists it ([#621](https://github.com/wayhouse-proxy/wayhouse/issues/621)) ([b05f718](https://github.com/wayhouse-proxy/wayhouse/commit/b05f7185e0a043c769885b6839d23050eff69b9c))
* **ci:** atomic keygen, latest never moves backwards, checksummed kubeconform ([#611](https://github.com/wayhouse-proxy/wayhouse/issues/611)) ([7efeeae](https://github.com/wayhouse-proxy/wayhouse/commit/7efeeaee3f2ec3a102751607ff61d0de6aa84400))
* **ci:** clear the open Trivy and CodeQL alerts ([#614](https://github.com/wayhouse-proxy/wayhouse/issues/614)) ([4e19d3c](https://github.com/wayhouse-proxy/wayhouse/commit/4e19d3c0acb6b6fb7397d198c043721e9472681f))
* **config:** validate refuses configs a proxy would refuse to boot ([#590](https://github.com/wayhouse-proxy/wayhouse/issues/590), [#598](https://github.com/wayhouse-proxy/wayhouse/issues/598)) ([#604](https://github.com/wayhouse-proxy/wayhouse/issues/604)) ([3857aa5](https://github.com/wayhouse-proxy/wayhouse/commit/3857aa54736734682f82205044c8a4b45c57f121))
* **controller:** a failed grant after a prefix claim releases the name ([#702](https://github.com/wayhouse-proxy/wayhouse/issues/702)) ([61d3e8c](https://github.com/wayhouse-proxy/wayhouse/commit/61d3e8ca86a0f785b09aa95c491846b67e735b66))
* **controller:** a prefix enrollment code cannot take over an enrolled name ([#686](https://github.com/wayhouse-proxy/wayhouse/issues/686)) ([3cf3b09](https://github.com/wayhouse-proxy/wayhouse/commit/3cf3b090035c354c13e44269567615e886622d12))
* **controller:** atomic prefix-name check; a taken name does not burn attempts ([#701](https://github.com/wayhouse-proxy/wayhouse/issues/701)) ([fbb4051](https://github.com/wayhouse-proxy/wayhouse/commit/fbb4051f9cfb56d3f0eab4e8b657f3bc98028e81))
* **controller:** BB1 safety follow-ups for feature overrides and plugin config edits ([#551](https://github.com/wayhouse-proxy/wayhouse/issues/551)) ([6bd772d](https://github.com/wayhouse-proxy/wayhouse/commit/6bd772ded74739064d88ed253bdc3e2b841d21b7))
* **controller:** confirm-and-revert for canary promote; a canary no longer ends a live trial ([#591](https://github.com/wayhouse-proxy/wayhouse/issues/591)) ([#608](https://github.com/wayhouse-proxy/wayhouse/issues/608)) ([0c91727](https://github.com/wayhouse-proxy/wayhouse/commit/0c917276cf1b565e253fe62f1e96d9ae310f2c35))
* **controller:** deleting an origin revokes its token and tunnel keys ([#682](https://github.com/wayhouse-proxy/wayhouse/issues/682)) ([8fa825e](https://github.com/wayhouse-proxy/wayhouse/commit/8fa825e7b5d2550279ea844660d701cd90c88a43))
* **controller:** keep enrollment from bricking on full stores ([#687](https://github.com/wayhouse-proxy/wayhouse/issues/687)) ([d2caaa2](https://github.com/wayhouse-proxy/wayhouse/commit/d2caaa2091e063115597ba7bc8e7762ffe2532df))
* **controller:** plugin hardening bundle before registry install (PH1) ([#567](https://github.com/wayhouse-proxy/wayhouse/issues/567)) ([d107d8d](https://github.com/wayhouse-proxy/wayhouse/commit/d107d8dd725d58310df81c335cf7e13b083b9160))
* **controller:** retry a key file that failed to load; secret PUTs check the active key ([#610](https://github.com/wayhouse-proxy/wayhouse/issues/610)) ([37f35ec](https://github.com/wayhouse-proxy/wayhouse/commit/37f35ecb6dc8bade56d2715f7522acfbef374069))
* **controller:** revoking an old spent code spares a re-enrolled host ([#699](https://github.com/wayhouse-proxy/wayhouse/issues/699)) ([28b298e](https://github.com/wayhouse-proxy/wayhouse/commit/28b298e6d1586a29857bfce8efc1fda5d8a20983))
* **controller:** rewrap cluster secrets on key rotation ([#603](https://github.com/wayhouse-proxy/wayhouse/issues/603)) ([fd9b59f](https://github.com/wayhouse-proxy/wayhouse/commit/fd9b59ff4aac08356d11e34f83e6ef0553f3eff3))
* **controller:** tunnel keys get their own cap; atomic key write on redeem ([#703](https://github.com/wayhouse-proxy/wayhouse/issues/703)) ([46e63e8](https://github.com/wayhouse-proxy/wayhouse/commit/46e63e80b34aa7fb131dd78fc0a694a79145609e))
* **core:** idle timeout for the intent, parent and plugin-overlay SSE readers ([#616](https://github.com/wayhouse-proxy/wayhouse/issues/616)) ([084e271](https://github.com/wayhouse-proxy/wayhouse/commit/084e271de16c4da99bd8816ee4f09a719c33766b))
* **core:** rate limiter per-net first; agent validates tunnel address ([#609](https://github.com/wayhouse-proxy/wayhouse/issues/609)) ([13dff16](https://github.com/wayhouse-proxy/wayhouse/commit/13dff16622d42c3701ac1baea21befd3fd056d15))
* **core:** secret store O_EXCL and lock; raft auth before protocol gate; override re-check ([#613](https://github.com/wayhouse-proxy/wayhouse/issues/613)) ([d7e1e95](https://github.com/wayhouse-proxy/wayhouse/commit/d7e1e957e704696a2acc69fb4ba0769f5d4b3058))
* **core:** SSE idle timeout for subscribers; per-source limiter survives a respawn ([#615](https://github.com/wayhouse-proxy/wayhouse/issues/615)) ([fd46aac](https://github.com/wayhouse-proxy/wayhouse/commit/fd46aac23cda3e29e4ddfb9e530933220ce47595))
* **deps:** bump wasmtime to 48.0.7 for CVE-2026-115893 ([#445](https://github.com/wayhouse-proxy/wayhouse/issues/445)) ([f258efa](https://github.com/wayhouse-proxy/wayhouse/commit/f258efab25b68ce4b7a430d41c21516316afbfc5))
* **enroll:** hand a replica's TLS key to systemd as a credential ([#691](https://github.com/wayhouse-proxy/wayhouse/issues/691)) ([e0e9a65](https://github.com/wayhouse-proxy/wayhouse/commit/e0e9a6595d0408462346e30b0058d45772bb6670))
* **enroll:** replica installs refuse a stray tls-key; verify-plugin tests ([#705](https://github.com/wayhouse-proxy/wayhouse/issues/705)) ([3fcd271](https://github.com/wayhouse-proxy/wayhouse/commit/3fcd271bac1f340b4125554419ac81f5d24ad042))
* H0b free-port test helper, redb write-rate measurement, ACME backoff on site change ([#557](https://github.com/wayhouse-proxy/wayhouse/issues/557)) ([56749b1](https://github.com/wayhouse-proxy/wayhouse/commit/56749b1599525a405f182ba91068cc3e22b78466))
* **http:** decide the plain-http bearer guard from url::Url ([#681](https://github.com/wayhouse-proxy/wayhouse/issues/681)) ([6ff2e7f](https://github.com/wayhouse-proxy/wayhouse/commit/6ff2e7f1db6900838e604594cc486b9599e59df4))
* **k8s:** one enrollment code Secret per role in the enrolled overlay ([#690](https://github.com/wayhouse-proxy/wayhouse/issues/690)) ([5d2bac4](https://github.com/wayhouse-proxy/wayhouse/commit/5d2bac4e6e27413b8c2c9433267a0061681aea2b))
* **plugins:** refused http request clears the old answer; bounded copies; repeated headers joined ([#622](https://github.com/wayhouse-proxy/wayhouse/issues/622)) ([495b79b](https://github.com/wayhouse-proxy/wayhouse/commit/495b79baea794265846989d9ddc65da095cc1f3b))
* **plugins:** stale tick record, HTTP vs epoch deadline, abandoned pool jobs ([#607](https://github.com/wayhouse-proxy/wayhouse/issues/607)) ([1db3906](https://github.com/wayhouse-proxy/wayhouse/commit/1db39068fb56f7804ac6a4912d2541c2763e8d23))
* **proxy:** a failed key fetch keeps the installed tunnel peer; one failed fetch per stream ([#700](https://github.com/wayhouse-proxy/wayhouse/issues/700)) ([64c3493](https://github.com/wayhouse-proxy/wayhouse/commit/64c349385798afb78f302751ee2826996d9bc70c))
* **proxy:** bound the sealed-PSK fetch in the tunnel-peer loop ([#683](https://github.com/wayhouse-proxy/wayhouse/issues/683)) ([72f37d0](https://github.com/wayhouse-proxy/wayhouse/commit/72f37d09f98f945f4f0c8de1bc7ed1a77b7b6a8a))
* **registry:** cross-kind duplicate conflict, canonical ABI, stable-only default pick ([#623](https://github.com/wayhouse-proxy/wayhouse/issues/623)) ([1ebfaab](https://github.com/wayhouse-proxy/wayhouse/commit/1ebfaab07c7c88d236932f41bd0736ebf46048fa))
* **secrets:** bound X-Actor, zeroize resolved secrets ([#624](https://github.com/wayhouse-proxy/wayhouse/issues/624)) ([cca7afc](https://github.com/wayhouse-proxy/wayhouse/commit/cca7afcf82e2002d760e1439f44e2c216f9e0f31))
* **secrets:** remove the temp store file on any write failure ([#626](https://github.com/wayhouse-proxy/wayhouse/issues/626)) ([92ef21f](https://github.com/wayhouse-proxy/wayhouse/commit/92ef21fd4a9c8eb8dd144c9454ffc3f3a5c36b68))
* **security:** bump undici, patch go-jose, drop curl from Caddy image ([#530](https://github.com/wayhouse-proxy/wayhouse/issues/530)) ([588fb2d](https://github.com/wayhouse-proxy/wayhouse/commit/588fb2da793918f1a1368b60c0d424b8d3a6b0d9))
* **ui:** demo-found problems: trial bar text, fleet name wrap, option labels ([#550](https://github.com/wayhouse-proxy/wayhouse/issues/550)) ([b270e10](https://github.com/wayhouse-proxy/wayhouse/commit/b270e109fe112f1f68f157f9d6b880c003f46643))
* **ui:** locked settings writes, rev on confirm/undo, viewer-first eviction ([#620](https://github.com/wayhouse-proxy/wayhouse/issues/620)) ([dc73500](https://github.com/wayhouse-proxy/wayhouse/commit/dc735005ecb02139c546d94b952dec4ddd44f558))
* **ui:** login delay no longer lets one client lock out a username ([#695](https://github.com/wayhouse-proxy/wayhouse/issues/695)) ([5d01419](https://github.com/wayhouse-proxy/wayhouse/commit/5d0141948d66e08a0aceb4165cfd4cfdf0170752))
* **ui:** make light-mode warn amber instead of brown ([#536](https://github.com/wayhouse-proxy/wayhouse/issues/536)) ([deb8805](https://github.com/wayhouse-proxy/wayhouse/commit/deb8805ee2edcee26dff754f4cbea389e5f35ac6))
* **ui:** no upstream URLs in 502s, escaped audit fields, no flag values for Viewers ([#619](https://github.com/wayhouse-proxy/wayhouse/issues/619)) ([55a2828](https://github.com/wayhouse-proxy/wayhouse/commit/55a28284985210a2802ce83f1179d31c1885d533))
* **ui:** open client routes from their URL ([#522](https://github.com/wayhouse-proxy/wayhouse/issues/522)) ([be91409](https://github.com/wayhouse-proxy/wayhouse/commit/be914097eeee05e3daa366ada6c4e153d056ca2e))
* **ui:** per-username login delay instead of a lockout bucket ([#629](https://github.com/wayhouse-proxy/wayhouse/issues/629)) ([a19c63a](https://github.com/wayhouse-proxy/wayhouse/commit/a19c63ad5f6465e8490e36a0e10db38cc58ec294))
* **ui:** refuse an empty or short --ui-password ([#592](https://github.com/wayhouse-proxy/wayhouse/issues/592)) ([#606](https://github.com/wayhouse-proxy/wayhouse/issues/606)) ([7bff85e](https://github.com/wayhouse-proxy/wayhouse/commit/7bff85e515e18b88533cfd056119707771f11e66))
* **ui:** tables, fleet and charts defects (UB-1) ([#552](https://github.com/wayhouse-proxy/wayhouse/issues/552)) ([143753b](https://github.com/wayhouse-proxy/wayhouse/commit/143753b8fa0fee09ba46857589207102254c6d38))
* **ui:** tighten sniffer registry trust (admin-only list, signed official, restore official) ([#605](https://github.com/wayhouse-proxy/wayhouse/issues/605)) ([cbd0f1b](https://github.com/wayhouse-proxy/wayhouse/commit/cbd0f1bdab61eceffe54c02e92a3806435d10a11))
* **ui:** UB-2 UI nit batch ([#498](https://github.com/wayhouse-proxy/wayhouse/issues/498), [#520](https://github.com/wayhouse-proxy/wayhouse/issues/520), [#523](https://github.com/wayhouse-proxy/wayhouse/issues/523), [#532](https://github.com/wayhouse-proxy/wayhouse/issues/532), [#535](https://github.com/wayhouse-proxy/wayhouse/issues/535), [#539](https://github.com/wayhouse-proxy/wayhouse/issues/539)) ([#558](https://github.com/wayhouse-proxy/wayhouse/issues/558)) ([e7f8204](https://github.com/wayhouse-proxy/wayhouse/commit/e7f8204a6edd7f95ac499539b3b5cffa455d0d2b))
* **ui:** UB-3 static serving, ACME write hardening, revoke gateway ([#523](https://github.com/wayhouse-proxy/wayhouse/issues/523), [#539](https://github.com/wayhouse-proxy/wayhouse/issues/539), [#542](https://github.com/wayhouse-proxy/wayhouse/issues/542)) ([#560](https://github.com/wayhouse-proxy/wayhouse/issues/560)) ([02f012b](https://github.com/wayhouse-proxy/wayhouse/commit/02f012b11bfb3020f0ec7816752a503a2fcaa7c3))
* **ui:** UP2 polish tail ([#546](https://github.com/wayhouse-proxy/wayhouse/issues/546), [#537](https://github.com/wayhouse-proxy/wayhouse/issues/537), [#548](https://github.com/wayhouse-proxy/wayhouse/issues/548), [#491](https://github.com/wayhouse-proxy/wayhouse/issues/491)) ([#563](https://github.com/wayhouse-proxy/wayhouse/issues/563)) ([7956a48](https://github.com/wayhouse-proxy/wayhouse/commit/7956a48a6a894676932a2218595a30a297f39a42))

## [0.3.0](https://github.com/wayhouse-proxy/wayhouse/compare/v0.2.0...v0.3.0) (2026-10-23)


### ⚠ BREAKING CHANGES

* **controller:** replace sled with redb ([#428](https://github.com/wayhouse-proxy/wayhouse/issues/428))

### Features

* **acme:** cargo feature to build the controller without ACME issuance ([#393](https://github.com/wayhouse-proxy/wayhouse/issues/393)) ([7f464a2](https://github.com/wayhouse-proxy/wayhouse/commit/7f464a2d90d612c2ce2e6b14fa6fb26b5dcb4621))
* **acme:** controller-managed ACME certificates, slice 1 (sites, sealed store, issuance loop) ([#389](https://github.com/wayhouse-proxy/wayhouse/issues/389)) ([c23a8a4](https://github.com/wayhouse-proxy/wayhouse/commit/c23a8a43b1df58ca24cbb51230d42cf5c38cfd3a))
* **acme:** external account binding and CNAME-delegated challenge zones ([#397](https://github.com/wayhouse-proxy/wayhouse/issues/397)) ([fb35d7c](https://github.com/wayhouse-proxy/wayhouse/commit/fb35d7cb350fe759d407c6ac82adee1eef68b757))
* **acme:** gateway tokens, bundle endpoints and wayhouse-certsync ([#395](https://github.com/wayhouse-proxy/wayhouse/issues/395)) ([51e95b6](https://github.com/wayhouse-proxy/wayhouse/commit/51e95b672eb89464e9d44db297e2504c356d339b))
* **acme:** HTTP-01 challenges answered from replicated entries ([#398](https://github.com/wayhouse-proxy/wayhouse/issues/398)) ([b81cd96](https://github.com/wayhouse-proxy/wayhouse/commit/b81cd963433c1ef22b0d34355af1b07a25dc1381))
* **acme:** on_acme_dns plugin trigger and DNS-01 solver ([#391](https://github.com/wayhouse-proxy/wayhouse/issues/391)) ([1434b8a](https://github.com/wayhouse-proxy/wayhouse/commit/1434b8af21f56c1e5b6018cbf9d27b8721fb22e5))
* **config:** web section with sites, upstreams and validation ([#371](https://github.com/wayhouse-proxy/wayhouse/issues/371)) ([fb4210d](https://github.com/wayhouse-proxy/wayhouse/commit/fb4210d3109b144c558aaf3f452144c8c79af354))
* **plugin:** port-based routing for plugins ([#432](https://github.com/wayhouse-proxy/wayhouse/issues/432)) ([8982e76](https://github.com/wayhouse-proxy/wayhouse/commit/8982e761ecd24a142cec927e84466836226f1828))
* **ui:** certificates page for ACME sites and gateways ([#400](https://github.com/wayhouse-proxy/wayhouse/issues/400)) ([49007ac](https://github.com/wayhouse-proxy/wayhouse/commit/49007ac8b8d459d2187dc753dbe3efcb1db9de5b))
* **web:** forward ACME HTTP-01 challenges to the controller ([#399](https://github.com/wayhouse-proxy/wayhouse/issues/399)) ([de03c5f](https://github.com/wayhouse-proxy/wayhouse/commit/de03c5ff27369348945b97c7ad4d37006c3be44c))
* **web:** licence notices, module licence check and web nits ([#387](https://github.com/wayhouse-proxy/wayhouse/issues/387)) ([ae6358e](https://github.com/wayhouse-proxy/wayhouse/commit/ae6358eb11f671501349179df0261f6176161b81))
* **web:** loopback listeners for via-wayhouse upstreams, GET /web and end-to-end tests ([#380](https://github.com/wayhouse-proxy/wayhouse/issues/380)) ([b0cb198](https://github.com/wayhouse-proxy/wayhouse/commit/b0cb1985e50c08aad627f770786d87e436230f31))
* **web:** wayhouse-web-sync and wayhouse-caddy image ([#384](https://github.com/wayhouse-proxy/wayhouse/issues/384)) ([827d91f](https://github.com/wayhouse-proxy/wayhouse/commit/827d91f09715b0c10b919c1fab8621c48fa50a6d))


### Bug Fixes

* **acme:** rewrap account and certificate keys on key rotation ([#413](https://github.com/wayhouse-proxy/wayhouse/issues/413)) ([9649d91](https://github.com/wayhouse-proxy/wayhouse/commit/9649d918ba55f8e14997a41dd2d8236d8995c045))
* **acme:** wait one poll interval before the first TXT lookup (mitigation) ([#407](https://github.com/wayhouse-proxy/wayhouse/issues/407)) ([bfdd6c6](https://github.com/wayhouse-proxy/wayhouse/commit/bfdd6c62ba314ba980a51f4d943d3ae630c4511d))
* certsync downgrade, repair and startup reload; registry IPv6 authority; fleet port claims ([#405](https://github.com/wayhouse-proxy/wayhouse/issues/405)) ([46b1b90](https://github.com/wayhouse-proxy/wayhouse/commit/46b1b9013cd9846c25a960bf0f79b96e74aead8d))
* close the stale-claim race, bound the PEM check, document certsync with native TLS ([#406](https://github.com/wayhouse-proxy/wayhouse/issues/406)) ([7c3895a](https://github.com/wayhouse-proxy/wayhouse/commit/7c3895a49c978612c8546eb1bf4db537fa212c20))
* **core:** rate limit the ACME bundle routes per source address ([#427](https://github.com/wayhouse-proxy/wayhouse/issues/427)) ([f8df3b6](https://github.com/wayhouse-proxy/wayhouse/commit/f8df3b63c3d19c16e031b3fef0b8f147e40a062b))
* **core:** review nits before 0.3.0 (rate limit, redb, port routes, UDP send path) ([#436](https://github.com/wayhouse-proxy/wayhouse/issues/436)) ([a9253c5](https://github.com/wayhouse-proxy/wayhouse/commit/a9253c595ae078ef641e47be5a9d801d6a822b98))
* **deps:** patch rustls and wasmtime advisories, document accepted unmaintained warnings ([#376](https://github.com/wayhouse-proxy/wayhouse/issues/376)) ([fe64ece](https://github.com/wayhouse-proxy/wayhouse/commit/fe64ecea5c1e88fae5c13f9c87e78b36fb4ad556))
* low-severity hardening from the 2026-10-22 review and web config nits ([#415](https://github.com/wayhouse-proxy/wayhouse/issues/415)) ([4284280](https://github.com/wayhouse-proxy/wayhouse/commit/42842801b05b3ce1d25a6c956d1eae26c5ed76f0))
* review follow-ups for ACME EAB, certsync, web-sync and the Certificates page ([#416](https://github.com/wayhouse-proxy/wayhouse/issues/416)) ([d1b2a0a](https://github.com/wayhouse-proxy/wayhouse/commit/d1b2a0a5c6eadb39d6dec47901901a85fd4c9448))
* **security:** build wayhouse-caddy with patched Go and x/net; bump vite ([#412](https://github.com/wayhouse-proxy/wayhouse/issues/412)) ([b917184](https://github.com/wayhouse-proxy/wayhouse/commit/b91718417d9dd0e967f614c8ca65ae7e06aa5a9a))


### Performance Improvements

* **core:** drop per-call heap allocations on the UDP send path ([#434](https://github.com/wayhouse-proxy/wayhouse/issues/434)) ([0051dd2](https://github.com/wayhouse-proxy/wayhouse/commit/0051dd2c7f2e73df899b2144d1b0e4f0ab9b26b0))


### Code Refactoring

* **controller:** replace sled with redb ([#428](https://github.com/wayhouse-proxy/wayhouse/issues/428)) ([a50d0d5](https://github.com/wayhouse-proxy/wayhouse/commit/a50d0d51ceb0169bdd5d00c8e843bcc6f7ca691a))

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
