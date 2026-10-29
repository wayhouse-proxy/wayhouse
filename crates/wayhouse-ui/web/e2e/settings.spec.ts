import { expect, test, type Page } from "@playwright/test";

const CONFIG = "# edge\nsettings:\n  workers: 2 # tasks\n";
const SCHEMA = {
  type: "object",
  properties: { settings: { type: "object", properties: { workers: { type: "integer" }, shutdown_grace_sec: { type: "integer" } } } },
};

/** A controller that holds one config, accepts submissions, and can put the last one on trial. */
async function stubBackend(page: Page, opts: { stale?: boolean; trialMs?: number } = {}) {
  const posts: { url: string; body: string }[] = [];
  let pendingUntil = 0;
  let reverted = false;
  await page.route("**/ui/session", (r) => r.fulfill({ status: 200, body: "ok" }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/config/schema", (r) => r.fulfill({ json: SCHEMA }));
  await page.route("**/api/ui/layout", (r) => r.fulfill({ status: 404, body: "" }));
  await page.route("**/api/config/validate", (r) => r.fulfill({ json: { ok: true, schema_version: 3 } }));
  await page.route("**/api/config/revert?*", (r) => {
    // The trial the operator saw, not whatever is on trial by now.
    reverted = new URL(r.request().url()).searchParams.get("revision") === "4";
    pendingUntil = 0;
    return r.fulfill({ status: 200, body: "" });
  });
  await page.route("**/api/config/pending", (r) => {
    const left = pendingUntil - Date.now();
    return r.fulfill({
      json: {
        pending: left > 0 ? { revision: 4, revert_to: 3, deadline_ms: pendingUntil, reasons: ["admin_listen"], remaining_ms: left } : null,
        now_ms: Date.now(),
        auto_confirm_secs: 60,
        risks: [{ id: "admin_listen", text: "settings.admin.listen changed" }],
      },
    });
  });
  await page.route(/\/api\/config(\?.*)?$/, async (r) => {
    if (r.request().method() === "POST") {
      posts.push({ url: r.request().url(), body: r.request().postData() ?? "" });
      if (opts.stale) return r.fulfill({ status: 409, json: { error: "config changed since revision 3" } });
      if (opts.trialMs) pendingUntil = Date.now() + opts.trialMs;
      return r.fulfill({ json: { revision: 4 } });
    }
    return r.fulfill({ status: 200, headers: { "x-config-revision": "3" }, body: CONFIG });
  });
  return { posts, wasReverted: () => reverted };
}

test("a form edit shows as a diff, and Apply posts the patched YAML against the loaded revision", async ({ page }) => {
  const { posts } = await stubBackend(page);
  await page.goto("/settings");
  const workers = page.getByLabel(/Worker tasks per listener/);
  await workers.fill("4");
  await page.getByRole("button", { name: "Apply" }).click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByLabel("Changes to the YAML")).toContainText("+  workers: 4 # tasks");
  expect(posts).toEqual([]);
  await dialog.getByRole("button", { name: "Apply" }).click();
  await expect(page.getByText("Applied as revision 4.")).toBeVisible();
  expect(posts).toHaveLength(1);
  expect(posts[0].url).toContain("base=3");
  expect(posts[0].body).toBe("# edge\nsettings:\n  workers: 4 # tasks\n");
});

test("a Source edit shows in the form", async ({ page }) => {
  await stubBackend(page);
  await page.goto("/settings/source");
  const editor = page.getByLabel("Configuration YAML");
  await editor.click();
  await page.keyboard.press("Control+A");
  await page.keyboard.type("settings:\n  workers: 7\n");
  await expect(page.getByText("1 unsaved change")).toBeVisible();
  await page.getByRole("link", { name: "General" }).click();
  await expect(page.getByLabel(/Worker tasks per listener/)).toHaveValue("7");
});

test("a stale base asks for a reload", async ({ page }) => {
  await stubBackend(page, { stale: true });
  await page.goto("/settings");
  await page.getByLabel(/Worker tasks per listener/).fill("5");
  await page.getByRole("button", { name: "Apply" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Apply" }).click();
  await expect(page.getByRole("alert")).toContainText("Someone else changed the configuration");
  await expect(page.getByRole("button", { name: "Reload the latest" })).toBeVisible();
});

test("a revision on trial shows the countdown on every page, and Revert now ends it", async ({ page }) => {
  const { wasReverted } = await stubBackend(page, { trialMs: 60_000 });
  await page.goto("/settings");
  await page.getByLabel(/Worker tasks per listener/).fill("5");
  await page.getByRole("button", { name: "Apply" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Apply" }).click();
  const bar = page.getByText(/Applied as revision 4/).first();
  await expect(bar).toContainText("reverts to revision 3");
  await page.getByRole("link", { name: "Config history" }).click();
  await expect(page.getByText(/Applied as revision 4/).first()).toBeVisible();
  await page.getByRole("button", { name: "Revert now" }).click();
  await expect(page.getByRole("button", { name: "Keep changes" })).toBeHidden();
  expect(wasReverted()).toBe(true);
});
