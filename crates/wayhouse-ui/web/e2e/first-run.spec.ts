import { expect, test } from "@playwright/test";

// The first run on a solo box: sign in, land on a blank-slate dashboard, and every page the
// bundle enables has an empty state that says what to do, not an error.
test("sign in, then walk the pages an empty box has", async ({ page }) => {
  let signedIn = false;
  const logins: string[] = [];
  await page.route("**/ui/session", (r) =>
    signedIn ? r.fulfill({ json: { authenticated: true, username: "admin", role: "admin" } }) : r.fulfill({ status: 401, json: { error: "login required" } }),
  );
  await page.route("**/ui/login", (r) => {
    logins.push(r.request().postDataJSON().password);
    if (r.request().postDataJSON().password !== "from-init-sh") return r.fulfill({ status: 401, json: { error: "wrong password" } });
    signedIn = true;
    return r.fulfill({ status: 200, body: "ok" });
  });
  await page.route("**/ws/**", (r) => r.abort());
  // Routes registered later win, so the catch-all goes first.
  await page.route("**/api/**", (r) => r.fulfill({ json: [] }));
  await page.route("**/api/plugin-routes", (r) => r.fulfill({ json: { routes: [], conflicts: [] } }));
  await page.route("**/api/fleet/nodes**", (r) =>
    r.fulfill({ json: { items: [], total: 0, page: 1, size: 25, rollup: { nodes: 0, degraded: 0, stale: 0, sessions: 0 }, groups: [] } }),
  );
  await page.route("**/api/plugins", (r) => r.fulfill({ json: [] }));
  await page.route("**/api/secrets", (r) => r.fulfill({ json: { items: [] } }));
  await page.route("**/api/ui/**", (r) => r.fulfill({ json: { items: [] } }));

  await page.goto("/");
  await page.getByLabel("Password").fill("not-it");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByText("wrong password")).toBeVisible();
  await page.getByLabel("Password").fill("from-init-sh");
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page.getByRole("navigation", { name: "Main" })).toBeVisible();
  expect(logins).toEqual(["not-it", "from-init-sh"]);

  const rail = page.getByRole("navigation", { name: "Main" });
  await rail.getByRole("link", { name: "Plugins" }).click();
  await expect(page.getByText(/No plugins installed yet/)).toBeVisible();
  await expect(page.getByRole("link", { name: "Install plugin" })).toBeVisible();

  await page.goto("/settings/secrets");
  await expect(page.getByRole("heading", { level: 1, name: "Secrets" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Add secret" })).toBeEnabled();
  await expect(page.getByRole("row")).toHaveCount(1); // the header only: nothing stored yet
});
