import { expect, test } from "@playwright/test";

const flag = (name: string, over: Record<string, unknown> = {}) => ({
  name,
  value: true,
  source: "default",
  default: true,
  scope: "proxy",
  apply: "live",
  override: "both",
  protection: false,
  wired: true,
  available: true,
  since: 4,
  risk: `${name} is lost`,
  impact: "Open connections are untouched.",
  ...over,
});

test("an admin switches a protection flag off for an hour with a reason, sees it on every node, then clears the override", async ({ page }) => {
  const puts: unknown[] = [];
  let overridden = false;
  await page.route("**/ui/session", (r) => r.fulfill({ status: 200, body: "ok" }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/config", (r) => r.fulfill({ body: "pools: []\n", headers: { "x-config-revision": "1" } }));
  await page.route("**/api/fleet/features", (r) =>
    r.fulfill({
      json: {
        reporting: 3,
        total: 3,
        flags: [{ name: "acl", on: overridden ? 0 : 3, off: overridden ? 3 : 0, unavailable: 0, off_instances: overridden ? ["eu-1", "eu-2", "us-1"] : [], overridden_instances: [] }],
      },
    }),
  );
  await page.route("**/api/features/overrides", (r) =>
    r.fulfill({ json: { items: overridden ? [{ name: "acl", value: false, reason: "false positives", actor: "ana", set_at: 1, expires_at: 4102444800 }] : [] } }),
  );
  await page.route("**/api/features/overrides/acl", (r) => {
    if (r.request().method() === "PUT") {
      puts.push(r.request().postDataJSON());
      overridden = true;
      return r.fulfill({ json: {} });
    }
    overridden = false;
    return r.fulfill({ status: 204, body: "" });
  });
  await page.route("**/api/features", (r) =>
    r.fulfill({ json: { flags: [], proxy_flags: [flag("acl", overridden ? { value: false, source: "override", protection: true } : { protection: true })] } }),
  );

  await page.goto("/features");
  await page.getByRole("button", { name: "Override acl" }).click();
  const dlg = page.getByRole("dialog");
  await expect(dlg.getByText("acl is lost")).toBeVisible();
  await dlg.getByLabel("Reason").fill("false positives");
  await dlg.getByLabel("Back on after").selectOption("3600");
  await dlg.getByRole("button", { name: "Turn off acl" }).click();
  await expect(dlg).toBeHidden();
  expect(puts).toHaveLength(1);
  expect(puts[0]).toMatchObject({ value: false, reason: "false positives", ttl_secs: 3600 });
  expect(puts[0]).not.toHaveProperty("expires_at");
  await expect(page.getByText(/Protection is reduced: acl is off/)).toBeVisible();
  await expect(page.getByText("off on 3 of 3 nodes")).toBeVisible();

  await page.getByRole("button", { name: "Clear override of acl" }).click();
  await page.getByRole("button", { name: "Yes, clear acl" }).click();
  await expect(page.getByText(/Protection is reduced/)).toHaveCount(0);
});
