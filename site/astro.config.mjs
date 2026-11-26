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
                        {
                            label: "Topics",
                            items: [{ autogenerate: { directory: "deep-dive/topics" } }],
                        },
                        // One entry per subfolder of docs/deep-dive/ (the folder name is not a nice label).
                        {
                            label: "Design chapters",
                            items: [{ autogenerate: { directory: "deep-dive/design" } }],
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
