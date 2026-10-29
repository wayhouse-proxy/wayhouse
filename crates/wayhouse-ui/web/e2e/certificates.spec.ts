import { expect, test } from "@playwright/test";

test("an admin adds an ACME site and registers a gateway, the token shows once", async ({ page }) => {
  const puts: { url: string; body: unknown }[] = [];
  await page.route("**/ui/session", (r) => r.fulfill({ status: 200, body: "ok" }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/acme/account", (r) => r.fulfill({ json: { directory_url: "https://ca.test/dir", account_url: null } }));
  await page.route("**/api/acme/gateways", (r) => {
    if (r.request().method() === "POST") return r.fulfill({ status: 201, json: { name: "edge-1", token: "tok-abc-123" } });
    return r.fulfill({ json: { gateways: [] } });
  });
  await page.route("**/api/acme/sites/*", (r) => {
    puts.push({ url: r.request().url(), body: r.request().postDataJSON() });
    return r.fulfill({ status: 200, body: "" });
  });
  await page.route("**/api/acme/sites", (r) => r.fulfill({ json: { sites: [] } }));

  await page.goto("/certificates");
  await page.getByRole("button", { name: "Add site" }).click();
  const dlg = page.getByRole("dialog");
  await dlg.getByLabel("Site name").fill("panel");
  await dlg.getByLabel(/^Names/).fill("panel.example.com");
  await dlg.getByLabel("Challenge", { exact: true }).selectOption("http-01");
  await dlg.getByRole("button", { name: "Add site" }).click();
  await expect(dlg).toBeHidden();
  expect(puts).toHaveLength(1);
  expect(puts[0].url).toContain("/api/acme/sites/panel");
  expect(puts[0].body).toEqual({ names: ["panel.example.com"], challenge: "http-01", gateways: [], enabled: true });

  await page.getByRole("button", { name: "Register gateway" }).click();
  await page.getByRole("dialog").getByLabel("Gateway name").fill("edge-1");
  await page.getByRole("dialog").getByRole("button", { name: "Register" }).click();
  await expect(page.getByLabel("Gateway token")).toHaveValue("tok-abc-123");
  await page.getByRole("button", { name: "Done" }).click();
  await expect(page.getByText("tok-abc-123")).toHaveCount(0);
});
