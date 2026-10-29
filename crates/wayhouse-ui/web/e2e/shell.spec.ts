import { expect, test, type Page } from "@playwright/test";

async function stub(page: Page, role: string) {
  await page.route("**/ui/session", (r) =>
    r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ authenticated: true, username: "ana", role }) }),
  );
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/**", (r) => r.fulfill({ status: 200, contentType: "application/json", body: "[]" }));
}

test("navigation moves between pages and marks the current one", async ({ page }) => {
  await stub(page, "admin");
  await page.goto("/fleet");
  const rail = page.getByRole("navigation", { name: "Main" });
  await expect(rail.getByRole("link", { name: "Fleet" })).toHaveAttribute("aria-current", "page");
  await rail.getByRole("link", { name: "Plugins" }).click();
  await expect(page).toHaveURL(/\/plugins$/);
  await expect(page.getByRole("heading", { level: 1, name: "Plugins" })).toBeVisible();
  await expect(page.getByRole("navigation", { name: "Breadcrumb" })).toHaveText("Configure / Plugins");
});

test("a viewer cannot install plugins; an admin can", async ({ page }) => {
  await stub(page, "viewer");
  await page.goto("/plugins");
  await expect(page.getByRole("heading", { level: 1, name: "Plugins" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Install plugin" })).toHaveCount(0);
  await stub(page, "admin");
  await page.reload();
  await expect(page.getByRole("link", { name: "Install plugin" })).toBeVisible();
});

test("the rail is reachable by keyboard", async ({ page }) => {
  await stub(page, "admin");
  await page.goto("/fleet");
  await page.getByRole("button", { name: "Settings" }).focus();
  await page.keyboard.press("Enter");
  await page.getByRole("link", { name: "Appearance" }).focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/\/settings\/appearance$/);
});
