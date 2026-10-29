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
