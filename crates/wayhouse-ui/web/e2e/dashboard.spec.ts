import { expect, test, type BrowserContext } from "@playwright/test";

type Board = { name: string; widgets: { id: string; width: number }[] };

/** The controller's /ui/dashboards, in memory, shared by every browser context a test opens. */
async function serve(ctx: BrowserContext, boards: Board[]) {
  await ctx.route("**/ui/session", (r) =>
    r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ authenticated: true, username: "ana", role: "operator" }) }),
  );
  await ctx.route("**/ws/**", (r) => r.abort());
  // The last route registered wins, so the catch-all goes first.
  await ctx.route("**/api/**", (r) => r.fulfill({ status: 200, contentType: "application/json", body: "[]" }));
  await ctx.route("**/api/acme/sites", (r) => r.fulfill({ status: 200, contentType: "application/json", body: '{"sites":[]}' }));
  await ctx.route("**/api/ui/dashboards**", async (r) => {
    const req = r.request();
    if (req.method() === "PUT") {
      const d = req.postDataJSON() as Board;
      const i = boards.findIndex((b) => b.name === d.name);
      if (i >= 0) boards[i] = d;
      else boards.push(d);
      return r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(d) });
    }
    return r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ items: boards }) });
  });
}

test("customize, Done, and a second browser sees the saved layout", async ({ browser }) => {
  const boards: Board[] = [{ name: "Ops", widgets: [{ id: "nodes.v1", width: 6 }, { id: "attention.v1", width: 6 }] }];
  const first = await browser.newContext();
  await serve(first, boards);
  const page = await first.newPage();
  await page.goto("/dashboard");
  await expect(page.getByRole("region", { name: "Nodes" })).toBeVisible();

  await page.getByRole("button", { name: "Customize" }).click();
  await page.getByRole("button", { name: "Move Nodes later" }).click();
  await page.getByRole("button", { name: "Remove Needs attention" }).click();
  await page.getByLabel("Add widget").selectOption("certificates.v1");
  await page.getByRole("button", { name: "Done" }).click();
  await expect(page.getByText("Dashboard saved", { exact: true })).toBeVisible();

  await page.reload();
  await expect(page.getByRole("region", { name: "Certificates" })).toBeVisible();
  await expect(page.getByRole("region", { name: "Needs attention" })).toHaveCount(0);

  const second = await browser.newContext();
  await serve(second, boards);
  const other = await second.newPage();
  await other.goto("/dashboard");
  await expect(other.getByRole("region", { name: "Certificates" })).toBeVisible();
  await expect(other.getByRole("region", { name: "Needs attention" })).toHaveCount(0);
  await first.close();
  await second.close();
});
