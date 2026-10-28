# Component specs

Every interactive component has these states, all styled from [tokens.css](tokens.css): default, hover, focus-visible, active, disabled, loading, error. A component that ships without all of them is not done. Focus is always `outline: var(--focus-ring)` with `var(--focus-offset)`, never removed. Principles and the do and don't list: [DESIGN.md](DESIGN.md).

Components are shadcn/ui source in `components/ui`, restyled to these specs. Names below are the shadcn names where one exists.

## Button

| Variant   | Use                                                 | Look                                                             |
| --------- | --------------------------------------------------- | ---------------------------------------------------------------- |
| Primary   | The one main action on a page or form. At most one. | `--primary` ground, `--primary-ink` text                         |
| Secondary | Everything else                                     | `--surface` ground, 1 px `--control-border`, ink text            |
| Danger    | Destructive or lock-out-risk actions                | Secondary look, text in `--bad`; filled only in the confirm step |
| Ghost     | Toolbar and row actions                             | No border, hover `--surface-sunken`                              |
| Small     | Inside tables, widget headers                       | 24 px high, 12 px text                                           |

32 px high, 3 px radius, 600 weight, 5 px by 11 px padding. Label is the action ("Apply", "Drain node"). Loading replaces the label with the same text plus a 14 px spinner, keeps its width, and sets `aria-busy`. Disabled uses `aria-disabled` and explains why in an adjacent hint or tooltip when the reason is not obvious.

## Form controls

**Field row.** Two columns: label and help on the left (240 px), control on the right. Stacks under 760 px. Label is 500 weight. Help is 12 px muted, one or two lines, says what the setting does and its default. The YAML key may follow in mono. A 6 px `--select` dot after the label marks a field changed from the saved value.

| Control          | Spec                                                                                                      |
| ---------------- | --------------------------------------------------------------------------------------------------------- |
| Text input       | 32 px, 1 px `--control-border`, `--surface` ground. Mono for addresses and keys.                          |
| Number input     | 110 px wide, unit as muted text on the right ("seconds", "connections", "per second").                    |
| Select           | Native `select` styled to match; Radix Select only when options need search or groups.                    |
| Switch           | 34 by 18 px, `role="switch"`, on = `--select` ground. Applies to boolean settings only.                   |
| Textarea         | Mono, for lists such as CIDRs, one per line.                                                              |
| Secret field     | Shows a secret by name (`secret: admin-token`) with Replace and Reveal-never; values are never read back. |
| Segmented / tabs | Only for switching a view, never to set a value.                                                          |

**Validation.** Validate on blur and on Apply, never while typing the first time. The message sits under the control in `--bad`, with an error shape, names the problem and the fix, and the control gets `aria-invalid` and `aria-describedby`. Server errors from the controller are shown at the field when they name one, otherwise in the save bar.

**Generated forms.** The schema renderer maps types to the controls above: boolean to Switch, enum to Select, integer or number with a unit annotation to Number input, array of strings to Textarea or tag list, object to a collapsible section. Fields marked advanced in the layout file appear only when Show advanced is on. Hand-built editors are used for routes, sources and web sites.

## Status

State is a shape, a colour and a word, always together.

| State                        | Shape                      | Colour        | Word     |
| ---------------------------- | -------------------------- | ------------- | -------- |
| Up, healthy, valid           | Round mark, 9 px           | `--good`      | Up       |
| Degraded, draining, expiring | Triangle mark, 11 by 10 px | `--warn`      | Degraded |
| Down, failed, invalid        | Square mark, 9 px          | `--bad`       | Down     |
| Unknown, disabled            | Hollow round mark          | `--ink-muted` | Unknown  |

The word is in ink, the mark in the state colour. A status never appears as a coloured pill with no text and never as colour on text alone. In dense tables the status cell is `mark + word`; in a summary bar the segments also carry counts.

## Table

32 px rows, 12 px muted semibold header on `--surface-sunken`, 1 px row separators, numbers right-aligned with tabular numerals, no zebra striping. Hover tints 4 percent of ink; the selected row uses `--select-bg` and `aria-selected="true"`. Rows that open the inspector are focusable and open on Enter.

- **Sorting.** Sortable headers are buttons with `aria-sort`; one sort at a time, shown with an arrow.
- **Grouping.** A group row (`--surface-sunken`, semibold) carries a disclosure, the group name, and a rollup (nodes, degraded count, connections). Groups can be collapsed. Large groups show the first rows and a "Show the other N nodes" row.
- **Paging.** Server-side. The pager below the table shows "Showing 1 to 25 of 500", previous and next, and a page-size select (25, 100).
- **Filters.** Above the table: text search, state, tag and region selects, saved filters. Active filters stay visible; "Clear filters" appears when any is set.
- **Overflow.** The table scrolls horizontally inside its own container; the page never scrolls sideways. Long cell text truncates with a title, except node names, which never truncate.
- **Empty and zero results.** See Empty, loading and error states.
- **Loading.** Skeleton rows at row height, not a spinner in the middle.

Large lists (above about 200 rows in one view) are virtualised.

## Panel and widget

A panel is a `--surface` box with a 1 px `--line` border and 3 px radius. Header: title (13 px, 600) at the left, optional tools at the right, 38 px high with a bottom hairline. Body: 12 px padding. Footer (optional): freshness or a link, 12 px muted, hairline above. Panels are not nested inside panels; inside a panel use hairlines and spacing.

**Widget** is a panel on the dashboard grid with a contract:

- It declares a **title**, a **default width** (small 4, medium 6, wide 12 columns) and the widths it supports.
- It shows an aggregate by default and the top N of a list, with the total ("Showing 5 of 49") and a link to the full view.
- It has **loading** (skeleton at its final size), **no data** ("No data for this range" with the cause when known), and **error** (what failed, a Retry button; other widgets keep working) states.
- Its footer says how fresh the data is ("Updated 4 s ago").
- It may declare settings (range, region filter, top N). Settings open in the inspector, not a modal.

Built-in widgets for the first version: Nodes (state bar with counts), Config rollout (revision spread), Certificates (soonest expiries), Connections over time, Throughput by region, Needs attention.

**Headline number.** One per widget, 26 px semibold tabular, with its unit or context in 13 px muted. It always sits above or beside its history (a bar or a chart), never alone as the whole widget.

### Dashboard grid and customize mode

12-column CSS grid, 12 px gap. Under 760 px every widget is full width.

Customize mode is entered with a **Customize** button and left with **Done**. In it each widget shows a dashed outline and tools in its header: move earlier, move later, width select, remove. Drag-and-drop may be added as an enhancement, but the buttons are the required, keyboard-accessible path. A toolbar offers **Add widget** (a select listing widgets not already on the dashboard), **Reset** (back to the saved dashboard) and **Show JSON**. Changes save when Done is pressed and are announced ("Dashboard saved"). A dashboard selector switches between named dashboards; create, rename and delete live in its menu.

A dashboard is stored in the controller and shared by all operators; export and import use the JSON shape `{ "name": "...", "widgets": [{ "id": "...", "width": 4 }] }`. Widget ids are stable and versioned; an unknown id imports as a placeholder with a "This widget is not available" message and can be removed.

## Charts

- **Scale.** Axes with a real scale: tick labels name values the chart reaches, a zero baseline for counts and rates, a 1 px `--line` grid.
- **Series.** Colours come from `--series-1..5`, in order, with direct labels or a legend. State colours are never used for series. The last point is emphasised with a mark.
- **Annotation.** Events that explain a change (incident, rollout) are a dashed vertical line with a short label. When a value changed, mark the onset.
- **Time.** The x axis is UTC unless the operator sets a zone; the range ("Last 60 minutes") is shown and selectable from the widget settings. The ring buffer holds about an hour at one-minute resolution; longer ranges are not offered until the aggregator keeps them.
- **Accessibility.** Every chart has `role="img"` with an `aria-label` that states what it shows and the key fact, plus a "View as table" toggle that shows the same data.
- **No decoration.** No gradients, no glow, no rounded caps, no 3D. An area fill is the series colour at low opacity.
- **Bars** (rank by region) have a visible value at the end of each bar and start at zero.

## Navigation and layout

**Rail.** Dark in both themes. Brand at the top, groups with a small uppercase muted label (the only place uppercase is used), items 32 px high with a 3 px radius. The current page has `aria-current="page"` and the `--rail-hover` ground with semibold text. A parent item with children toggles with `aria-expanded` and a chevron; children are indented 16 px and shown in muted ink until current. Sign out sits at the bottom.

**Page tabs.** Text tabs with a 2 px `--select` underline on the selected one; `role="tablist"`, arrow keys move, content panels are labelled. Tabs hold parts of one subject; they are not steps.

**Collapsible section.** `details`/`summary` pattern: a 600 weight title, a chevron, a hairline between header and content, open by default unless marked advanced. The summary shows a count when something inside is changed ("2 changed").

**Inspector.** Right-hand panel, 20 rem, `--surface` ground with a left hairline. Header: the item name in mono and a Close button. Then status, a key-value list (muted terms, tabular values), tabs where the item has more parts (Routes, Config, Events), then actions. Selecting another row replaces its content; Escape or Close dismisses it. Under 1100 px it sits below the page instead of beside it. Focus moves to the inspector heading when it opens and returns to the row when it closes.

**Alert bar.** One line under the page header for the single most important condition ("1 node degraded: ..."), with the state mark, a sentence, and a link. Not a banner stack; more than one condition becomes "3 conditions need attention" with a link.

## Settings pages

Anatomy, top to bottom: breadcrumb, title and summary (including the schema version the page covers), Show advanced switch, tabs, collapsible sections of field rows, and a sticky save bar.

**Save bar.** Sticky at the bottom of the page. Left: "No changes" or "N unsaved changes". Right: **Discard** and **Apply** (primary), both disabled when nothing changed. Apply sends the change to the controller as one revision.

**Confirm and revert (lock-out protection).** Settings and controller changes that could cut the operator off (client address source, trusted networks, listener ports, admin token) apply in **trial** mode: the save bar turns to the warning border and reads "Applied. Confirm within 60 s, or the controller reverts to revision N." with a live countdown (announced at 30, 10 and 0 seconds, not every second), **Keep changes** and **Revert now**. If the countdown ends, or the session is lost, the controller reverts. The countdown is driven by the controller, not the browser, so closing the tab still reverts.

**Process flags.** Settings that are process flags (restart needed) show a "Restart required" mark on the field, and Apply offers "Apply and restart" with the supervisor named. Anything not applied live is marked until the restart finishes.

**Secrets.** See Secret field. The page lists a secret by name and last-changed time only.

**Config history.** Every Apply is a revision; the history page shows the diff in YAML with the author and time, and offers Revert to this revision.

## Dialog and confirmation

Prefer inline. Destructive and bulk actions expand a confirm block in place ("Drain ams-edge-01? New connections stop. Existing ones finish or time out after 10 minutes.") with a Danger button that names the action and a Cancel. Use a dialog (Radix Dialog) only when the task needs interruption or protected focus: install a plugin with a trust prompt, enter a secret, review a multi-step change. A dialog has a title, a body that states the consequence, the action as a verb, a Cancel, a 6 px radius, the overlay shadow, a focus trap, and restores focus on close. Bulk actions state the count ("Drain 12 nodes matching this filter").

## Empty, loading and error states

| State        | Spec                                                                                                                                                                                              |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| First use    | Left-aligned in the content area: one sentence on what will appear and the one action that fills it ("Add a node to see it here"). No illustration.                                               |
| No results   | "No nodes match these filters." with Clear filters. Never an empty table body.                                                                                                                    |
| Loading      | Skeleton at the final size and layout; no centred spinner. After 8 seconds add "Still loading".                                                                                                   |
| Error        | What failed, the likely cause, the next step, and Retry: "The aggregator did not answer. Check its address in Settings, then retry." Keep stale data visible, marked as stale, when there is any. |
| Disconnected | A persistent alert bar: "Live updates stopped. Showing data from 14:31." with Reconnect.                                                                                                          |
| Forbidden    | "The admin token was rejected." with a link to sign in again.                                                                                                                                     |

## Routes and relationships

Listener, pool and target relationships are drawn as patch lines: orthogonal 1.5 px `--control-border` lines from left to right with a state mark at each end (round for up, square for down). Labels are 12 px ink, ports and counts in mono muted. The diagram scrolls horizontally in its own container on narrow screens and has an `aria-label` that spells out each route and its state; the same data is available as a table.
