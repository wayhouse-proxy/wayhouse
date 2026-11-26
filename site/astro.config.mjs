// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import starlightLinksValidator from "starlight-links-validator";

// The pages under src/content/docs/ are generated from ../docs/ by scripts/sync_docs.py
// (run by `npm run build`); edit the Markdown in docs/, never the generated copy.
export default defineConfig({
    site: "https://wayhouse-proxy.dev",
    integrations: [
        starlight({
            title: "wayhouse",
            description: "Game-agnostic reverse proxy for game servers.",
            social: [
                {
                    icon: "github",
                    label: "GitHub",
                    href: "https://github.com/wayhouse-proxy/wayhouse",
                },
            ],
            sidebar: [
                {
                    label: "Manual",
                    collapsed: false,
                    items: [
                        { slug: "manual" },
                        // One entry per subfolder of docs/manual/, in reading order (a folder name is not a label).
                        {
                            label: "Getting started",
                            items: [{ autogenerate: { directory: "manual/getting-started" } }],
                        },
                        {
                            label: "Routing and clients",
                            items: [{ autogenerate: { directory: "manual/routing" } }],
                        },
                        {
                            label: "Pools and backends",
                            items: [{ autogenerate: { directory: "manual/pools" } }],
                        },
                        {
                            label: "Security and limits",
                            items: [{ autogenerate: { directory: "manual/security" } }],
                        },
                        {
                            label: "Operating",
                            items: [{ autogenerate: { directory: "manual/operate" } }],
                        },
                        {
                            label: "Certificates",
                            items: [{ autogenerate: { directory: "manual/certificates" } }],
                        },
                        {
                            label: "Resolvers, plugins and sniffers",
                            items: [{ autogenerate: { directory: "manual/extend" } }],
                        },
                        {
                            label: "Web sites",
                            items: [{ autogenerate: { directory: "manual/web" } }],
                        },
                        {
                            label: "Admin UI",
                            items: [{ autogenerate: { directory: "manual/ui" } }],
                        },
                    ],
                },
                {
                    label: "Deep dive",
                    collapsed: false,
                    items: [
                        { slug: "deep-dive" },
                        // Every topic page is listed once, in a themed group. `make docs-deep-dive` fails on a topic that is missing here.
                        {
                            label: "Design",
                            collapsed: false,
                            items: [
                                { autogenerate: { directory: "deep-dive/design" } },
                            ],
                        },
                        {
                            label: "The data plane",
                            collapsed: true,
                            items: [
                                { slug: "deep-dive/topics/data-plane-listeners" },
                                { slug: "deep-dive/topics/udp-sessions" },
                                { slug: "deep-dive/topics/filters-and-limits" },
                                { slug: "deep-dive/topics/routing-and-matchers" },
                                { slug: "deep-dive/topics/pools-and-load-balancing" },
                                { slug: "deep-dive/topics/health-checking" },
                                { slug: "deep-dive/topics/backend-discovery" },
                                { slug: "deep-dive/topics/gossip-health" },
                                { slug: "deep-dive/topics/connection-pump" },
                                { slug: "deep-dive/topics/proxy-protocol" },
                                { slug: "deep-dive/latency-ledger" },
                            ],
                        },
                        {
                            label: "Configuring and operating a proxy",
                            collapsed: true,
                            items: [
                                { slug: "deep-dive/topics/config-schema" },
                                { slug: "deep-dive/topics/snapshot-and-hot-reload" },
                                { slug: "deep-dive/topics/admin-api" },
                                { slug: "deep-dive/topics/runtime-feature-flags" },
                                { slug: "deep-dive/topics/process-settings" },
                                { slug: "deep-dive/topics/metrics-and-logs" },
                                { slug: "deep-dive/topics/secrets" },
                            ],
                        },
                        {
                            label: "Extending the proxy",
                            collapsed: true,
                            items: [
                                { slug: "deep-dive/topics/sniffers" },
                                { slug: "deep-dive/topics/external-resolvers" },
                                { slug: "deep-dive/topics/plugin-host" },
                                { slug: "deep-dive/topics/plugin-runtime-in-controller" },
                                { slug: "deep-dive/topics/registries-and-release-feed" },
                            ],
                        },
                        {
                            label: "The control plane",
                            collapsed: true,
                            items: [
                                { slug: "deep-dive/topics/controller-core" },
                                { slug: "deep-dive/topics/controller-ha" },
                                { slug: "deep-dive/topics/config-rollout" },
                                { slug: "deep-dive/topics/enrollment" },
                                { slug: "deep-dive/topics/aggregator" },
                                { slug: "deep-dive/topics/all-in-one-control" },
                            ],
                        },
                        {
                            label: "The tunnel",
                            collapsed: true,
                            items: [
                                { slug: "deep-dive/topics/tunnel-client" },
                                { slug: "deep-dive/topics/tunnel-agent" },
                                { slug: "deep-dive/topics/tunnel-registry" },
                            ],
                        },
                        {
                            label: "Web sites and certificates",
                            collapsed: true,
                            items: [
                                { slug: "deep-dive/topics/web-gateway" },
                                { slug: "deep-dive/topics/acme" },
                            ],
                        },
                        {
                            label: "The console",
                            collapsed: true,
                            items: [
                                { slug: "deep-dive/topics/ui-backend" },
                                { slug: "deep-dive/topics/ui-frontend" },
                                { slug: "deep-dive/topics/ui-data" },
                                { slug: "deep-dive/topics/http-toolkit" },
                            ],
                        },
                        {
                            label: "Building and shipping",
                            collapsed: true,
                            items: [
                                { slug: "deep-dive/topics/ci-pipeline" },
                                { slug: "deep-dive/topics/release-pipeline" },
                                { slug: "deep-dive/topics/deployment-artifacts" },
                                { slug: "deep-dive/topics/testing-and-benchmarks" },
                                { slug: "deep-dive/topics/docs-site" },
                            ],
                        },
                    ],
                },
                {
                    label: "Reference",
                    collapsed: true,
                    items: [{ autogenerate: { directory: "reference" } }],
                },
            ],
            // Fails the build on a broken internal link or anchor.
            customCss: ["./src/styles/custom.css"],
            plugins: [starlightLinksValidator({ errorOnLocalLinks: false })],
        }),
    ],
});
