import { expect, test } from "@playwright/test";

// Cluster secrets (K1-UI): add, list by name, never show the value, replace warning, delete.
test("add a secret, see it by name only, then delete it", async ({ page }) => {
  const store = new Map<string, string>();
  const puts: Array<{ name: string; value: string }> = [];
  await page.route("**/ui/session", (r) => r.fulfill({ json: { authenticated: true, username: "ana", role: "admin" } }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/secrets", (r) =>
    r.fulfill({ json: { items: [...store.keys()].map((name) => ({ name, key_id: "k1", set_at: 1_800_000_000, actor: "ana" })) } }),
  );
  await page.route("**/api/secrets/*", async (r) => {
    const name = decodeURIComponent(new URL(r.request().url()).pathname.split("/").at(-1)!);
    if (r.request().method() === "PUT") {
      const value = r.request().postDataJSON().value as string;
      puts.push({ name, value });
      store.set(name, value);
      return r.fulfill({ status: 200, json: { name, key_id: "k1", set_at: 1_800_000_000, actor: "ana" } });
    }
    store.delete(name);
    return r.fulfill({ status: 204, body: "" });
  });

  await page.goto("/settings/secrets");
  await expect(page.getByRole("heading", { level: 1, name: "Secrets" })).toBeVisible();
  await page.getByRole("button", { name: "Add secret" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Name").fill("admin-token");
  await expect(dialog.getByRole("button", { name: "Save secret" })).toBeDisabled();
  await dialog.getByLabel("Value").fill("correct-horse-battery");
  await dialog.getByRole("button", { name: "Save secret" }).click();

  const row = page.getByRole("row", { name: /admin-token/ });
  await expect(row).toBeVisible();
  expect(puts).toEqual([{ name: "admin-token", value: "correct-horse-battery" }]);
  // Write-only: the value is nowhere on the page.
  await expect(page.locator("body")).not.toContainText("correct-horse-battery");

  // A name that exists says it replaces.
  await page.getByRole("button", { name: "Add secret" }).click();
  await page.getByRole("dialog").getByLabel("Name").fill("admin-token");
  await expect(page.getByRole("dialog")).toContainText("exists");
  await expect(page.getByRole("dialog").getByRole("button", { name: "Replace secret" })).toBeDisabled();
  await page.getByRole("dialog").getByRole("button", { name: "Cancel" }).click();

  await page.getByRole("button", { name: "Delete admin-token" }).click();
  await page.getByRole("button", { name: "Yes, delete" }).click();
  await expect(page.getByRole("row", { name: /admin-token/ })).toHaveCount(0);
});
