# UI form layout

The web UI draws configuration forms from the JSON Schema that `GET /config/schema` serves. A **layout** groups, orders and explains the fields. It never changes what the YAML says: the form still edits the same keys, and the file stays the source of truth. A default layout ships with the UI; an admin can override parts of it per deployment (Settings > Layout, stored in the controller).

## Format

```json
{
    "version": 1,
    "fields": {
        "settings.workers": {
            "label": "Worker tasks per listener",
            "group": "General",
            "order": 1,
            "unit": "tasks",
            "page": "global"
        },
        "settings.admin.auth_token": { "label": "Admin API token", "widget": "secret" },
        "listeners.*.bind": { "label": "Address", "help": "host:port this listener accepts on." }
    }
}
```

The shipped default is keyed by the dotted path of a field (`pools.*.targets`, `*` for any list index). An **override** is keyed by JSON Pointer, as the controller stores it (`/pools/0/targets`; a number or `*` in the pointer matches any index), and carries only `help`, `group`, `order`, `widget`, `advanced` and `page`. `label` and `unit` are default-only: the controller rejects them with a 422. An override replaces keys field by field over the default: a field you do not mention keeps its default layout.

| Key        | Type    | Meaning                                                                                                                        |
| ---------- | ------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `label`    | string  | The name shown for the field; the YAML key follows it in mono text.                                                            |
| `help`     | string  | Help text under the label. Without it the schema's own description (its first paragraph) is used.                              |
| `group`    | string  | Collapsible section title inside the parent. Fields (and sub-objects) with the same group share one section.                   |
| `order`    | number  | Smaller first. Fields without an order follow the ordered ones in schema order.                                                |
| `widget`   | string  | `secret` (password input), `lines` (one value per line), `select`, `switch`, `number` or `text`. The default follows the type. |
| `unit`     | string  | Muted text after a number. Without it the unit is read from the name (`_ms`, `_sec`, `_bytes`, `_per_sec`).                    |
| `advanced` | boolean | Shown only when "Show advanced" is on. Advanced fields are never needed for a working configuration.                           |
| `page`     | string  | Which settings page shows the field (and the sections that contain it).                                                        |

## Rules

- A path that is not a field of the configuration is ignored with a warning. It is never an error that stops editing.
- An unknown key or a value of the wrong type in one field is ignored with a warning; the rest of the layout applies.
- A layout that is not JSON, has no `"version": 1` or has no `fields` object is not used at all: the forms use the default layout and show a notice saying why.
- Settings > Layout (admins write, everyone reads) edits the override as JSON with a live preview on one real form, import, export and Reset to default.
- The override is limited to 64 KiB. Export and import use the same JSON, so a layout is a file as well as a setting.

## Related

[Design system](design-system/DESIGN.md) (section and field-row patterns), [configuration reference](05-configuration.md).
