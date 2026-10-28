# Themes

Wayhouse ships two built-in themes, **Light** and **Dark**, and supports **custom themes** that operators define for their deployment (house colours, a high-contrast variant, a brand). Tokens: [tokens.css](tokens.css). Contrast rules: [accessibility.md](accessibility.md).

## Model

A theme is a name plus a value for each themed token in section 1 of [tokens.css](tokens.css). Components only read semantic tokens, so a theme never touches component code. The active theme is set on the root element as `data-theme="light"`, `"dark"`, or `"custom"`. With no attribute, the UI follows the operating system (light or dark). A custom theme is applied as variable overrides on `:root` (and `color-scheme` is set from its declared base), so every component picks it up without a rebuild.

```json
{
    "name": "Night shift",
    "base": "dark",
    "tokens": {
        "bg": "#101216",
        "surface": "#171a20",
        "select": "#7fb0ff"
    }
}
```

`base` is `light` or `dark`. Tokens left out fall back to the base theme, so a theme can change a handful of values.

## What a theme may change

| Group           | Tokens                                                                          |
| --------------- | ------------------------------------------------------------------------------- |
| Ground and text | `bg`, `surface`, `surface-sunken`, `ink`, `ink-muted`, `line`, `control-border` |
| Navigation rail | `rail`, `rail-ink`, `rail-ink-muted`, `rail-hover`                              |
| Interaction     | `select`, `select-bg`, `primary`, `primary-ink`                                 |
| Data series     | `series-1` to `series-5`                                                        |
| State colours   | `good`, `warn`, `bad` (hue may shift; see the limits below)                     |

A theme may **not** change: type families and scale, spacing, row and control heights, radius, motion, the state shapes and words, layout, or focus-ring width. Those are the system's structure and keep the product recognisable and operable. Fonts are not themable in the first version.

## Validation

A theme can be previewed freely but is saved only if it passes these checks, run in the browser and again by the controller on save. Failures name the token pair and the ratio (or the colour difference). The implementation is `src/lib/theme/validate.ts` in the web project; the controller's Rust port must give the same result on `src/lib/theme/fixtures.json`.

1. `ink` on `bg`, `surface` and `surface-sunken`: at least 4.5:1.
2. `ink-muted` on `bg`, `surface` and `surface-sunken`: at least 4.5:1.
3. `rail-ink` and `rail-ink-muted` on `rail` and `rail-hover`: at least 4.5:1.
4. `select` on `surface`, and `ink` on `select-bg`: at least 4.5:1.
5. `primary-ink` on `primary`: at least 4.5:1.
6. `good`, `warn`, `bad` on `bg`, `surface` and `surface-sunken`: at least 4.5:1.
7. `control-border` on `surface` and `bg`: at least 3:1.
8. `good`, `warn` and `bad` must stay distinguishable from each other: pairwise CIEDE2000 difference of at least 10, in normal vision and under simulated deuteranopia, protanopia and tritanopia (Machado et al. 2009 matrices at full severity, applied in linear RGB), because shape and word are the primary signal and colour reinforces them.
9. `series-1` to `series-5`: each at least 3:1 on `surface`, and pairwise at least 10 apart by the same measure as check 8 (all four vision types).
10. `select` must differ from `good`, `warn` and `bad` (CIEDE2000 of at least 10, normal vision) so selection never reads as state.

A theme that fails any check cannot be saved or set as the default. The built-in themes are checked by the same code.

## Where themes live and who picks

- Custom themes are stored in the controller, shared by all operators of a deployment, with JSON import and export (the same "UI and file are equivalent" rule as dashboards).
- The default theme for the deployment is set in Settings. Each browser can override it with its own choice (System, Light, Dark, or any saved custom theme), remembered locally; a person who picks nothing gets the deployment default.
- Deleting a theme that is in use falls back to System.

## Editor

Settings > Appearance lists themes, each with a live swatch strip and a "Preview" action that applies the theme to the whole UI for this session only. The editor starts from a built-in theme (Duplicate), shows each token with its colour value and its checks inline (pass or fail with the ratio), and uses the same field patterns as the rest of Settings. A "Reset to base" action removes a token override. A contrast failure is shown as text with a mark, not colour alone.

## Open items

- Whether the series palette is themable per theme or derived from `select` and a fixed hue set.
- Controller endpoints and schema for themes (to be added to the UI overhaul design).
