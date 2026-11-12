import { expect, test } from "@playwright/test";

const SHA = "ab".repeat(32);
const CAPS = { triggers: { on_timer: true }, tick_interval_secs: 60, log: true, state: null };
const PANEL = { ...CAPS, http: { hosts: [{ host: "p.example" }] } };
const MANIFEST = {
  name: "bound",
  bindings: [{ option: "panel", kind: "hostname" }],
  options: { type: "object", required: ["panel"], properties: { panel: { type: "string", title: "Panel host" } } },
};

test("install wizard: upload, options, review, approve, install", async ({ page }) => {
  const installs: any[] = [];
  await page.route("**/ui/session", (r) => r.fulfill({ status: 200, body: "ok" }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/plugin-routes", (r) => r.fulfill({ json: { routes: [], conflicts: [] } }));
  await page.route("**/api/plugins", async (r) => {
    const req = r.request();
    if (req.method() === "GET") return r.fulfill({ json: installs.map((i) => ({ ...i, id: "0123456789abcdef", size: 1, sha256: SHA, approved: i.approved, enabled: true, created_at: 1, created_by: null })) });
    const body = req.postDataJSON();
    if (body.dry_run) return r.fulfill({ json: { effective: PANEL } });
    installs.push(body);
    return r.fulfill({ status: 201, json: {} });
  });
  await page.route("**/api/plugins/modules", (r) =>
    r.fulfill({ status: 201, json: { sha256: SHA, size: 4, abi: "0.2", capabilities: CAPS, manifest: MANIFEST } }),
  );

  await page.goto("/plugins");
  await page.getByRole("link", { name: "Install plugin" }).click();
  await page.getByLabel(".wasm file").setInputFiles({ name: "bound.wasm", mimeType: "application/wasm", buffer: Buffer.from([0, 97, 115, 109]) });
  await page.getByRole("button", { name: "Inspect module" }).click();
  await page.getByLabel(/Panel host/).fill("p.example");
  await page.getByRole("button", { name: "Review what it asks for" }).click();
  expect(installs).toEqual([]);
  await page.getByRole("button", { name: "Install…" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("Makes HTTPS requests to p.example");
  await dialog.getByRole("checkbox", { name: /I approve/ }).check();
  await dialog.getByRole("button", { name: "Approve and install" }).click();
  await expect(page.getByText("installed bound")).toBeVisible();
  expect(installs).toHaveLength(1);
  expect(installs[0]).toMatchObject({ approved: PANEL, config: { panel: "p.example" } });
});

const ADMIN = { authenticated: true, username: "ana", role: "admin" };

test("an install the plugins file owns is badged, read-only, and has a plan", async ({ page }) => {
  await page.route("**/ui/session", (r) => r.fulfill({ json: ADMIN }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/plugin-routes", (r) => r.fulfill({ json: { routes: [], conflicts: [] } }));
  await page.route("**/api/plugins", (r) =>
    r.fulfill({ json: [{ id: "0123456789abcdef", name: "from-file", sha256: SHA, size: 1, approved: CAPS, config: {}, enabled: true, created_at: 1, created_by: "plugins-file" }] }),
  );
  await page.route("**/api/plugins/file/plan", (r) =>
    r.fulfill({ json: { applied: false, plugins: [{ name: "next-one", id: null, action: "refused", reason: "the approval hash does not match", effective_sha256: "cd".repeat(32) }] } }),
  );

  await page.goto("/plugins");
  const row = page.getByRole("row", { name: /from-file/ });
  await expect(row.getByText("managed by file")).toBeVisible();
  await expect(row.getByRole("button", { name: "Delete from-file" })).toHaveAttribute("aria-disabled", "true");
  await page.getByRole("button", { name: "Show file plan" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog).toContainText("next-one");
  await expect(dialog).toContainText("cd".repeat(32));
});

test("registry install: list, fetch an unsigned module only after confirming, land in the wizard", async ({ page }) => {
  const fetches: any[] = [];
  const REG = { id: "r1", name: "wayhouse plugins", url: "https://example.test/index.json", official: true, risk: "official" };
  await page.route("**/ui/session", (r) => r.fulfill({ json: ADMIN }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/plugin-registries", (r) => r.fulfill({ json: { registries: [REG], persistent: true } }));
  await page.route("**/api/plugin-registries/r1/plugins", (r) =>
    r.fulfill({
      json: {
        registry: REG,
        index_name: "plugins",
        host_abi: "0.2",
        plugins: [{ name: "bound", description: "Panel sync", license: "MIT", categories: [], tags: [], latest: "0.2.0", abi: "0.2", signed: false, updated: "2026-11-12", abi_compatible: true }],
      },
    }),
  );
  await page.route("**/api/plugin-registries/r1/fetch", (r) => {
    const body = r.request().postDataJSON();
    fetches.push(body);
    if (!body.allow_unsigned) return r.fulfill({ status: 409, json: { error: "signature could not be verified", needs_confirm: true, risk: "official", signed: false } });
    return r.fulfill({ json: { plugin: "bound", version: "0.2.0", signed: false, risk: "official", upload: { sha256: SHA, size: 4, abi: "0.2", capabilities: CAPS, manifest: MANIFEST } } });
  });

  await page.goto("/plugins/new");
  await page.getByText("From a registry").click();
  await page.getByRole("button", { name: /Fetch/ }).first().click();
  await expect(page.getByRole("dialog")).toBeVisible();
  expect(fetches).toHaveLength(1);
  // Nothing more is sent until the risk is accepted.
  await expect(page.getByRole("dialog").getByRole("button", { name: "Fetch anyway" })).toBeDisabled();
  expect(fetches).toHaveLength(1);
  await page.getByRole("dialog").getByRole("checkbox", { name: /accept this risk/ }).check();
  await page.getByRole("dialog").getByRole("button", { name: "Fetch anyway" }).click();
  await expect(page.getByLabel(/Panel host/)).toBeVisible();
  expect(fetches.map((f) => f.allow_unsigned)).toEqual([false, true]);
});
