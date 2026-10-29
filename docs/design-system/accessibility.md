# Accessibility

Baseline: WCAG 2.2 level AA for every screen, plus the project rules below. An operations console is used for hours, on bad days, often by keyboard, so these are not optional extras. Tokens: [tokens.css](tokens.css). Components: [components.md](components.md).

## Rules

1. **Never colour alone.** State is a shape, a colour and a word. Selection is a tint plus `aria-selected` and, in the rail, semibold text. A changed field is a dot plus an accessible "changed" description.
2. **Contrast.** Text 4.5:1 (large text 3:1). Borders of controls and the graphic parts of charts 3:1 against their surface. Decorative separators (`--line`) may be lower because they carry no meaning.
3. **Keyboard.** Everything works without a pointer, in a logical order. Focus is always visible: 2 px `--select` outline with 1 px offset (on the rail the outline is `--rail-ink`). No keyboard traps; dialogs trap focus and restore it. Rows that open the inspector are focusable and open on Enter. Drag-and-drop always has a button alternative.
4. **Targets.** At least 24 by 24 px everywhere (WCAG 2.2 target size), and 44 by 44 px under a coarse pointer (the tokens switch row and control height).
5. **Semantics first.** Use real elements: `button`, `a`, `table`, `th scope`, `label`, `fieldset`, `details`. ARIA only where no element exists: `role="switch"` with `aria-checked`, `aria-sort`, `aria-current="page"`, `aria-expanded`, `aria-selected`, `role="tablist"`.
6. **Live updates.** Live regions (`role="status"`, polite) announce completed actions and the confirm countdown at 30, 10 and 0 seconds. Streaming numbers (connections, throughput) are not live regions.
7. **Motion.** Motion only shows state change and is off under `prefers-reduced-motion` (the tokens set durations to ~0). Nothing flashes.
8. **Charts.** Every chart has a text alternative that states the key fact and a "View as table" toggle. Series are distinguishable without colour (direct labels or a legend with marks).
9. **Zoom and reflow.** Works at 200 percent zoom and at 320 px width without horizontal page scroll; wide tables and diagrams scroll inside their own container. Text uses rem.
10. **Forms.** Every control has a visible label. Help and errors are tied with `aria-describedby`. Errors name the problem and the fix, and `aria-invalid` is set. Required state is stated in words.
11. **Language and names.** Icon-only buttons have an `aria-label`. Page titles are `h1`; sections follow the heading order. Page `<title>` names the page.
12. **Custom themes** are validated against these contrast rules before they can be saved ([themes.md](themes.md)).

## Verified contrast of the built-in themes

Computed with the WCAG relative-luminance formula, by script, on the values in [tokens.css](tokens.css) (2026-10-28). Ratios are rounded down to two decimals; where a token is used on several grounds, the lowest ratio is shown.

| Pair                                                         | Light | Dark  | Needs |
| ------------------------------------------------------------ | ----- | ----- | ----- |
| `--ink` on `--bg`                                            | 15.24 | 15.02 | 4.5   |
| `--ink` on `--surface`                                       | 16.47 | 13.83 | 4.5   |
| `--ink-muted` on `--bg`                                      | 5.12  | 6.07  | 4.5   |
| `--ink-muted` on `--surface`                                 | 5.54  | 5.59  | 4.5   |
| `--ink-muted` on `--surface-sunken`                          | 4.73  | 5.40  | 4.5   |
| `--select` on `--surface`                                    | 5.58  | 7.22  | 4.5   |
| `--ink` on `--select-bg` (selected row)                      | 14.25 | 11.73 | 4.5   |
| `--good` on `--bg`, `--surface`, `--surface-sunken` (lowest) | 4.56  | 7.05  | 4.5   |
| `--warn` on `--bg`, `--surface`, `--surface-sunken` (lowest) | 3.10  | 9.90  | 3     |
| `--bad` on `--bg`, `--surface`, `--surface-sunken` (lowest)  | 4.94  | 4.66  | 4.5   |
| `--rail-ink` on `--rail`                                     | 14.06 | 15.51 | 4.5   |
| `--rail-ink-muted` on `--rail`                               | 5.34  | 6.32  | 4.5   |
| `--rail-ink-muted` on `--rail-hover`                         | 4.69  | 5.07  | 4.5   |
| `--primary-ink` on `--primary`                               | 16.47 | 15.02 | 4.5   |
| `--select` on `--select-bg`                                  | 4.83  | 6.13  | 4.5   |
| `--control-border` on `--surface` and `--bg` (lowest)        | 3.52  | 4.00  | 3     |
| `--series-1..5` on `--surface` (lowest)                      | 3.38  | 4.82  | 3     |

Colour-vision check (2026-10-28, `src/lib/theme` in the web project): `--good`, `--warn` and `--bad` are pairwise at least 10 apart (CIEDE2000, see [themes.md](themes.md) check 8) in normal vision and under simulated deuteranopia, protanopia and tritanopia; so are `--series-1..5` (check 9) and `--select` against the three state colours in normal vision (check 10). `--warn` is amber (about 40 degrees hue), not brown, and is held to the 3:1 graphic ratio because it is only ever drawn as a mark, border or icon with its word in `--ink`; at 4.5:1 a light-ground yellow is forced to brown. The first palette failed this (light `--warn` and `--bad` were 1.7 apart under deuteranopia); the state and series values in [tokens.css](tokens.css) were re-picked. Light `--series-4` and `--series-5` (first olive and dark brown-grey) are now violet and raspberry, so no series sits in the muddy olive-brown band. Shape and word remain the primary signal for state; series also carry direct labels or a legend.

Treat the table as a regression test: `validate.test.ts` checks the built-in themes against the same rules (and the values in `builtin.ts` against `tokens.css`), so a token change that breaks one fails CI. Recompute the table when a token changes.

## Review checklist

- [ ] Tab through the page: order is logical, focus is always visible, nothing is skipped or trapped.
- [ ] Operate every control with the keyboard only, including customize mode and the inspector.
- [ ] Turn off colour (greyscale): every state is still readable.
- [ ] 200 percent zoom and 320 px wide: no clipped text, no page-level horizontal scroll.
- [ ] Screen reader pass on one page: landmarks, headings, table headers, status announcements, chart alternatives.
- [ ] `prefers-reduced-motion`: no motion remains.
- [ ] Both themes, and each custom theme the change touches.
- [ ] Error, empty, loading and disconnected states exist and make sense.
