# Stored UI data

The web UI keeps a few records in the controller so every operator of a deployment sees the same thing: named **dashboards**, custom **themes**, saved fleet **filters**, an optional **form layout** override, and the deployment **defaults** (default dashboard and theme). The controller stores JSON and checks its shape, size and names; what a widget id or a token means is the UI's business, so an unknown widget id is kept (the UI shows a placeholder).

Under HA the records are written through Raft like plugin installs and readable on every replica. A standalone controller keeps them in its own database. Nothing here is a secret: do not put credentials in a dashboard, filter or layout.

## Roles

The controller checks only its bearer token. The UI backend (`/api/ui/*`) holds that token and applies the roles:

| Records                                | Read   | Write    |
| -------------------------------------- | ------ | -------- |
| dashboards, saved filters              | viewer | operator |
| themes, form layout override, defaults | viewer | admin    |

## Controller API

| Route                                             | What                                                                                      |
| ------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `GET /ui/dashboards`, `/ui/themes`, `/ui/filters` | `{"items": [...]}`, each record with its `name`.                                          |
| `GET`/`PUT`/`DELETE /ui/dashboards/{name}`        | One dashboard. The export shape is `{"name", "widgets": [{"id", "width", "settings"?}]}`. |
| `GET`/`PUT`/`DELETE /ui/themes/{name}`            | One theme: `{"name", "base": "light"\|"dark", "tokens": {"bg": "#101216", ...}}`.         |
| `GET`/`PUT`/`DELETE /ui/filters/{name}`           | One saved fleet filter (a JSON object).                                                   |
| `GET`/`PUT`/`DELETE /ui/layout`                   | The form layout override. `404` means "use the shipped default".                          |
| `GET`/`PUT /ui/defaults`                          | `{"dashboard"?, "theme"?}`; an empty object when nothing is set.                          |

A `PUT` replaces the record. The name in the path wins over a `name` in the body. Answers: `200` with the record, `204` on delete, `404` unknown record, `409` the collection is full, `422` malformed or over a limit (the text names the path, for example `widgets[0].width must be an integer from 1 to 12`).

## Limits

| Record       | Largest | Most records |
| ------------ | ------- | ------------ |
| dashboard    | 64 KiB  | 100          |
| theme        | 8 KiB   | 50           |
| saved filter | 4 KiB   | 200          |
| form layout  | 64 KiB  | 1            |
| defaults     | 1 KiB   | 1            |

Names are 1 to 64 characters without `/` or control characters. A dashboard holds at most 100 widgets, each `width` 1 to 12 (the grid has 12 columns). A theme may not be named `light`, `dark` or `system` (the built-ins); `defaults.theme` may be one of those or a saved theme. Deleting the dashboard or theme that is the default clears that default on every replica.

A theme is also held to the ten contrast checks of `docs/design-system/themes.md`. A theme that fails any answers `422` with `error` (the first failure in words, for example `ink on bg is 1.60:1, needs 4.5:1.`) and `failures`, a list of `{check, pair, value, needed, message}` for every failed check. The checks run on the node that takes the request (a follower refuses a failing theme itself, and the leader checks again before it proposes the write); replicas do not repeat them when they apply the entry. The Rust port in `ui_data/contrast.rs` and the web validator both pass the cases in `crates/wayhouse-ui/web/src/lib/theme/fixtures.json`.

## Form layout format (version 1)

The override is merged over the layout the UI ships. Paths are JSON Pointers into the config document.

```json
{
    "version": 1,
    "fields": {
        "/settings/admin/listen": {
            "group": "Admin",
            "order": 2,
            "widget": "text",
            "advanced": true,
            "page": "Global settings",
            "help": "Shown under the field."
        }
    }
}
```

Every key inside a field is optional; an unknown key or a wrong type is a `422` naming the path (`fields./settings/admin/listen.order must be an integer`). A path that matches no field is not an error: the UI ignores it with a warning, so a layout survives a schema change.

## Upgrading under HA

Like plugin installs, a write is refused until every replica runs a build that decodes UI data entries (`409` naming the node). Upgrade every controller before the first save. Reads are always served.
