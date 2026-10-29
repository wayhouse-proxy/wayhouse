# Wayhouse UI design system

Status: decided with Wueschli on 2026-10-28; the 0.4.0 UI overhaul builds it (foundation in `crates/wayhouse-ui/web`, `src/tokens.css` is a copy of [tokens.css](tokens.css) that a test keeps identical). The look is called **Switchboard**.

| File                                 | What it holds                                                                                                      |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------ |
| [DESIGN.md](DESIGN.md)               | This file: principles, do and don't, foundations, layout, scale rules, copy                                        |
| [tokens.css](tokens.css)             | Every colour (light and dark), type, space, radius, elevation and motion token, mapped to Tailwind 4               |
| [components.md](components.md)       | Specs for buttons, forms, tables, status, panels and widgets, charts, navigation, settings, empty and error states |
| [accessibility.md](accessibility.md) | Baseline, verified contrast numbers, keyboard and motion rules, a checklist                                        |
| [themes.md](themes.md)               | Built-in and custom themes: what a theme may change, validation, storage                                           |
| [preview.html](preview.html)         | A static page that renders the tokens and core components; open it in a browser                                    |

## Stack

React 19, TypeScript, Vite, Tailwind 4 and shadcn/ui on Radix primitives. shadcn/ui components are copied into the repo (`crates/wayhouse-ui/web/src/components/ui`) and restyled to these tokens, so there is no component-library upgrade cycle. Schema-generated forms use `@rjsf/shadcn` (decision 2A of the overhaul design). Tables use TanStack Table; the chart library is still to be chosen (see Open items). The UI stays a set of static files served by the Rust `wayhouse-ui` process: Node is a build-time dependency only.

## What this UI is

An operations console. People open it to check a fleet, find what is wrong and change configuration. It is not a marketing page. Familiarity beats novelty: a person who has used OPNsense or Proxmox should know where everything is before noticing the look. The identity lives in type, palette, density and one signature move; never in the layout, the navigation model or the controls, which stay standard.

The fleet can be deployed worldwide. Every screen is designed for about 500 nodes and must degrade gracefully beyond that. A screen that works for 6 nodes and breaks at 500 is not finished.

## Principles

1. **Aggregate first, detail on demand.** Lead with counts and rollups ("451 of 500 up"), then groups, then single nodes. Never one card or one chart per node.
2. **State is never colour alone.** Every state is a colour, a shape and a word (see Status in components.md).
3. **Same thing, same look.** One button style per role, one form-control vocabulary, one icon style. If "Save" looks different in two places, one of them is wrong.
4. **UI and file are equivalent.** What the UI edits is what the YAML says. Show real field names, real units and real defaults. A layout file may group and order fields; it never changes meaning.
5. **Say what is about to happen.** Risky changes (lock-out risk, drain, delete) show their consequence in the control, and anything that could lock an operator out gets the 60-second confirm and automatic revert.
6. **Restraint with colour.** Cobalt means "selected" or "focused". Ink means "primary action". Green, amber and red mean state, nothing else.

## Foundations

**Palette.** A warm grey ground (`--bg`), a lighter `--surface` for panels, tables and inputs, near-black ink, and a dark ink navigation rail in both themes (a second neutral layer that gives the shell its identity). Primary buttons are ink, not accent. Cobalt (`--select`) marks selection and keyboard focus only. State colours follow convention: `--good` green, `--warn` amber, `--bad` red (selection cobalt doubles as info; it never marks state). Neutrals lean warm on purpose: pure grey reads as unconsidered. Exact values and contrast are in [tokens.css](tokens.css) and [accessibility.md](accessibility.md).

**Light and dark.** Both ship, follow the system by default, and can be switched by the person. Dark is designed, not inverted: surfaces step lighter with elevation, text is slightly off-white, and status colours are re-picked for contrast. The rail is dark in both.

**Type.** Public Sans for all UI text. IBM Plex Mono only for machine data: addresses, ports, hashes, YAML, node names, version strings. Never use mono as a decorative label face. Tabular numerals (`font-variant-numeric: tabular-nums`) wherever digits line up. A fixed rem scale from 12 to 20 px (`--fs-*`); one headline number per widget at 26 px. No fluid type. Body measure for prose and help text is 70ch.

**Space.** A 4 px base (`--space-*`). Related things sit close; sections separate with more space above than below. Table rows are 32 px, controls 32 px. Under a coarse pointer both become 44 px.

**Shape and depth.** 3 px radius on everything interactive and on panels, 6 px on overlays. Separation comes from 1 px hairlines, not shadows. Only overlays (menus, popovers, dialogs) get a shadow, and it has a real offset and blur.

**Icons.** One library, one stroke weight (Lucide, 1.5 px stroke, 16 px), always paired with a text label unless the control is a universally understood glyph with an accessible name. No emoji, no unicode glyphs as icons.

**Motion.** Only to show a state change: a section opening, a toggle moving, a value updating. 120 to 250 ms, ease-out. No page-load choreography, no decorative animation. All motion is off under `prefers-reduced-motion`.

**Signature move.** Relationships are drawn as thin orthogonal "patch lines": listener, pool, targets, with a state mark at each end (a round mark for up, a square mark for down). It appears in route and pool detail views and is the only illustration-like element in the product.

## Do and don't

| Do                                                                           | Don't                                                                       |
| ---------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| Group nodes by region with rollups; page and filter on the server            | Render a card or chart per node                                             |
| Use plain, standard controls: select, switch, number input, tabs             | Invent controls for flavour, or restyle scrollbars and native pickers       |
| Use hairlines and spacing to separate                                        | Stack shadows, glows, or nest panels inside panels                          |
| Put the headline number in a widget once, with its history                   | Use the big-number-small-label-three-stats-and-an-accent tile template      |
| Use one accent (cobalt) for selection and focus                              | Use gradients, gradient text, glass or blur as decoration                   |
| Use a heading as the label of a section                                      | Put a small uppercase kicker above a heading, or number sections            |
| Write controls as the action they do ("Apply", "Drain node")                 | Write "Submit", "OK", or vague errors                                       |
| Use an inline confirmation for drain, delete and similar                     | Open a modal for a task that needs neither interruption nor protected focus |
| Draw icons from the one icon set                                             | Use emoji or mixed icon styles                                              |
| Colour a chart series from the series palette                                | Reuse green, amber or red for series; those mean state                      |
| Mark an alert with its state mark and a word                                 | Use a thick coloured side stripe on cards, alerts or list items             |
| Use sparklines only when they carry real data with a scale and an onset mark | Use sparklines or progress rings as filler                                  |

This table is the checklist for review. A component that breaks a "Don't" is rejected, whoever wrote it.

## Layout

**Shell.** Three zones: the navigation rail on the left, the page in the middle, and an optional inspector on the right (20 rem). The rail is 13.75 rem wide, dark in both themes, and collapses to a top bar under 760 px. The inspector appears when a row is selected and sits below the page under 1100 px.

**Hierarchy.** Deep configuration needs depth, and each level has one job:

1. **Rail groups** (Operate, Configure, System): the top-level areas.
2. **Rail items**, with an optional second level that expands in place (Fleet > Nodes, Tags and regions; Settings > Network, Security, Secrets).
3. **Page tabs** split one area into parts of equal weight (Addresses, Limits, Timeouts).
4. **Collapsible sections** group related fields inside a tab. Sections are open by default except those marked advanced.
5. **Show advanced** (a page-level switch) reveals expert fields. Advanced fields are never required for a working configuration.
6. **Inspector** shows the details and quick actions for the selected row without leaving the list.

Breadcrumbs ("Settings / Network") sit above the page title. A page never needs more than these levels; deeper structure becomes a separate page.

**Page anatomy.** Breadcrumb, title and one-line summary, page-level actions at the right, optional alert, filters, content, pager or save bar. The page title is a plain heading.

**Dashboard.** The first page after sign-in. A 12-column grid of widgets (small = 4 columns, medium = 6, wide = 12). Customize mode lets the operator move, resize, remove and add widgets and switch between named dashboards. Dashboards are stored in the controller and shared by all operators (there is one admin token and no user accounts), with JSON export and import so a dashboard is a file too. Specs are in components.md.

## Scale rules (about 500 nodes, more possible)

- Lists are grouped (region, then node) with a rollup row per group: node count, degraded count, connections.
- Tables page on the server (25, 100 per page). Filtering and sorting run on the server. Always show "N of M".
- Widgets show aggregates and the top N with a link to the full list ("Showing 5 of 49").
- Search and filters are first-class: text search, state, tag and region filters, and named saved filters.
- Nodes carry tags and a region. A tags and labels model on nodes is a controller feature the overhaul must add.
- Bulk actions work on a filter result, with the count shown before confirming.
- Charts use aggregated series from the aggregator ring buffer; no chart needs a point per node.

## Writing

- Say what a thing does in the operator's words: "Connections per IP", not "max_conn_ip". The YAML key appears as secondary text where it helps.
- Controls name their action. Errors name the problem and the recovery: "The controller did not answer within 5 seconds. Check the controller address, then retry."
- Empty states teach: say what will appear here and the one action that fills it.
- Sentence case everywhere. No exclamation marks, no emoji.
- In public repo text and docs, refer to the maintainer by handle (Wueschli).

## Open items

- **Chart library.** Decided: Recharts through the shadcn chart pattern (E2 measures the bundle; fallback uPlot).
- **Tags and labels on nodes**, **dashboard storage endpoints in the controller**, and **custom theme storage** need entries in the overhaul design and the controller API.
- **Schema-form layout file.** The layout file (group, order, widget, help text) should reference the section and field patterns here.
