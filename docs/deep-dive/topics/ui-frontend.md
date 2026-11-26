---
title: The UI frontend
description: How the operator console is built as a static single-page app with no server of its own, why its forms are generated from the config's JSON Schema, how themes, dashboards and roles work in the browser, and how a whole demo runs with no backend.
---

The browser half of the console is a React application that is built once and served as plain files by [the UI backend](ui-backend.md). It has no server side code of its own. It draws what the backend relays, and it edits the same configuration that the proxies read. This page explains the choices that shape it. The screens are in the [Manual](../../manual/index.md), and the generated [Reference](../../reference/index.md) lists the stored records. The look is written down in the design system under `docs/design-system/`.

## The problem

1. **The console must keep up with the config.** Every new option should appear in the editor without a UI change, or the UI is always behind.
2. **It must scale to a big fleet.** The design target is about 500 nodes, and a screen that works for 6 and breaks at 500 is not finished.
3. **It must be safe in a browser.** There are no tokens in the page, and the roles only hide what the server would refuse anyway.
4. **Operators differ.** Dashboards, themes and layouts should be shared by a team, and a theme must stay readable.
5. **It must build without disturbing the Rust workspace,** and be demonstrable without a running fleet.

## A static app on a small, boring stack

The frontend is React with TypeScript, built by Vite, styled by Tailwind 4, with shadcn components copied into the repository on Radix primitives and restyled to the design tokens. **There is no component library upgrade cycle,** since the components are owned code. Tables use TanStack Table with virtualisation for long lists, charts use Recharts, and the YAML editor uses CodeMirror.

It is a **standalone `npm` project** and deliberately not a Cargo workspace member, for the same reason the fuzz crate is standalone: the build needs a different toolchain, and `make check` on the Rust workspace must not require Node. Node is a build time dependency only. The output is a directory of static files, which the backend serves as a fallback under every route it does not claim.

State comes from a query cache with sensible operational defaults: a stale read is retried once and refetched on focus, with a 5 second stale time. The live fleet view arrives through a WebSocket hook that connects to the backend's `/ws/fleet`, with the session cookie carried by the browser on the upgrade, and reconnects after a short delay. It does not replay anything, because the backend already holds the last known view. A message that is not an array of instances, such as a notice that no aggregator is configured, is ignored and a frame that cannot be parsed is dropped instead of crashing the dashboard.

## Forms generated from the schema

The editors for config are **generated from the config's JSON Schema**, the same document the parser's types produce (see [the config schema](config-schema.md)), rendered with `react-jsonschema-form`. That is the answer to the first problem: a new field in the config appears in the editor when the schema is regenerated.

The schema as `schemars` emits it is not what a form renders well, so a small function reshapes it without changing its meaning. "Or null" becomes an optional field, a union of string constants becomes one enum, a port that is an integer or a string becomes a string field marked so that it is converted back, a value that is plaintext or a `{ secret: name }` reference becomes one text field marked as a secret reference, and defaults move into an annotation so that the form can show them without writing them. The `x-wayhouse-since` annotation from the schema is kept, which is how the editor can say which schema version a field needs.

The schema alone does not say how to present a field, so an optional **layout** document groups, orders, labels and explains fields. It is keyed by dotted path with `*` for any list index or map key, and each entry can set a label, help text, a group, an order, a widget (secret, lines, select, switch, number or text), a unit, whether the field is advanced, and the settings page that shows it. A layout **never changes what the YAML says**. The principle behind the console is that the UI and the file are equivalent: it shows real field names, real units and real defaults. The layout that ships is a JSON file, and a deployment can store an override in the controller (see [stored UI data](ui-data.md)), merged over the shipped one.

Edits go through a shared document hook that holds the config text, applies patches to it and tracks the revision it started from, so a save sends the base revision and a concurrent change is refused instead of overwritten (see [the controller core](controller-core.md)). The editors for listeners and lists keep their logic in plain modules that are tested without the framework. Risky changes show their consequence in the control, and anything that could lock an operator out gets the confirm window with its automatic revert (see [config rollout](config-rollout.md)).

## Roles hide, the server refuses

The session gives the page a username and a role. A helper answers "is the role at least this", and buttons the role cannot use are hidden. The comment on the helper is the whole policy: **the server gates stay the authority, and the browser only hides what would be refused.** Destructive actions, such as a drain, removing a backend, rolling back a config or removing a sniffer, ask for confirmation first and never call the API when cancelled. Restorative actions, such as undrain, do not ask. A button disables while its request is in flight.

## Themes that stay readable

A theme is a base, light or dark, and a set of colour tokens. Choices are "system", "light", "dark" or a saved custom theme, remembered per browser in local storage with a fallback when storage is unavailable, and the deployment can set a default. Because anyone with the right role can save a theme, a theme is held to **ten contrast checks**, and the validator in the page is a port of the same checks that run in the controller (see [stored UI data](ui-data.md)). Both pass the same shared test cases, so the page can tell an author what fails before the server would refuse it.

## Dashboards

A dashboard is a list of placed widgets, each with a stable versioned id, a width in columns of a 12 column grid (small, medium and wide are 4, 6 and 12) and optional settings, up to 100 widgets. An unknown widget id is kept and shown as a placeholder, which is why the controller can store dashboards without understanding them. Until somebody saves one, a first time visitor sees a default that is not stored. Saving sends the revision it started from, so two editors do not silently overwrite each other.

## The fleet page and aggregation first

The first design principle is aggregate first, detail on demand: lead with counts and rollups, then groups, then single nodes, and never one card or one chart per node. The node list is filtered, sorted, grouped by region and paged **on the server**, and the URL holds the filter state, so a view can be shared. State is never colour alone: each state is a colour, a shape and a word. Motion exists only to show a state change, and it is off under `prefers-reduced-motion`.

## Tests and the demo

The unit tests run on `jsdom` with Testing Library and mock the API module at its boundary, so they need no running backend. They pin the behaviours that matter most: confirmation before destruction, buttons that disable while in flight, and the error and header handling in the API layer. The Rust half of that contract, that every proxied route forwards status and headers, is a test in the backend. End to end tests drive a real browser with Playwright and run accessibility checks through axe.

**A demo with no server.** A separate build replaces `fetch` and `WebSocket` with an in memory backend, so the real UI runs against mock data with no fleet. State lives in memory and resets on reload. The mock is built on the real config schema, so the forms are the real forms. The build folds everything, script, styles and fonts, into one self contained HTML file.

Fonts are self hosted, with a test that checks it, so the console makes no request to a third party.

## What was traded away

- **Generated forms are plainer** than hand made ones. The layout file recovers most of the polish and it cannot make a field mean something new.
- **Role hiding can lag the server.** A button may show for a role that the server would refuse, in which case the user sees the error. The server is right and the page is a convenience.
- **Two validators for themes,** one in the controller and one in the page. A shared fixtures file keeps them in step.
- **A second toolchain.** Node and npm are needed to build the UI, which is the price of keeping them out of the Rust build.
- **Client routes with a dot** in them would 404 from the backend's static rules, so routes are named without dots.

## Where to look

- `crates/wayhouse-ui/web/src/lib/form/`: the schema reshaping and the layout.
- `crates/wayhouse-ui/web/src/components/form/SchemaForm.tsx`: the generated forms.
- `crates/wayhouse-ui/web/src/lib/session.tsx`: the session and role helper.
- `crates/wayhouse-ui/web/src/lib/theme/`: themes and their validation.
- `crates/wayhouse-ui/web/src/lib/dashboard/`: the dashboard model.
- `crates/wayhouse-ui/web/src/lib/fleet/` and `useFleetSocket.ts`: the fleet view.
- `crates/wayhouse-ui/web/src/demo/`: the in memory demo backend.
- `docs/design-system/`: principles, tokens, components and accessibility.
