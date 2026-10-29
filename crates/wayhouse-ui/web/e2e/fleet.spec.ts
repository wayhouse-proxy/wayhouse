import { expect, test, type Page } from "@playwright/test";

const REGIONS = ["eu", "us", "ap"];
const FLEET = Array.from({ length: 500 }, (_, i) => ({
  instance: `n${String(i).padStart(3, "0")}`,
  state: i % 100 === 99 ? "stale" : i % 25 === 0 ? "degraded" : "ok",
  last_seen_ms_ago: 1000 + i,
  region: REGIONS[i % 3],
  tags: { env: i % 2 ? "prod" : "stage" },
  version: "0.4.0",
  protocol: "1.1",
  skew: "none",
  sessions: { tcp: 2, udp: 1 },
  backends: 1,
  unhealthy_backends: i % 25 === 0 ? 1 : 0,
}));

const rollup = (ns: typeof FLEET) => ({ nodes: ns.length, degraded: ns.filter((n) => n.state === "degraded").length, stale: ns.filter((n) => n.state === "stale").length, sessions: ns.length * 3 });

async function stub(page: Page, drained: string[]) {
  await page.route("**/ui/session", (r) => r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ authenticated: true, username: "ana", role: "admin" }) }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/ui/**", (r) => r.fulfill({ status: 200, contentType: "application/json", body: '{"items":[]}' }));
  await page.route("**/api/fleet/instances/*/drain", (r) => {
    drained.push(r.request().url().split("/").at(-2)!);
    return r.fulfill({ status: 200, contentType: "text/plain", body: "ok" });
  });
  await page.route("**/api/fleet/nodes**", (r) => {
    const url = new URL(r.request().url());
    const p = url.searchParams;
    const all = FLEET.filter((n) => (!p.get("state") || n.state === p.get("state")) && (!p.get("q") || n.instance.includes(p.get("q")!))).sort((a, b) => a.region.localeCompare(b.region) || a.instance.localeCompare(b.instance));
    const body = url.pathname.endsWith("/resolve")
      ? { count: all.length, instances: all.map((n) => n.instance) }
      : { items: all.slice((Number(p.get("page") ?? 1) - 1) * 25, Number(p.get("page") ?? 1) * 25), total: all.length, page: Number(p.get("page") ?? 1), size: 25, rollup: rollup(all), groups: REGIONS.slice().sort().map((region) => ({ region, ...rollup(all.filter((n) => n.region === region)) })) };
    return r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
  });
}

test("a 500-node fleet: page, filter, inspect and drain what a filter selects", async ({ page }) => {
  const drained: string[] = [];
  await stub(page, drained);
  await page.goto("/fleet");
  await expect(page.getByText("Showing 1 to 25 of 500")).toBeVisible();
  await page.getByRole("button", { name: "Next" }).click();
  await expect(page.getByText("Showing 26 to 50 of 500")).toBeVisible();
  await expect(page).toHaveURL(/page=2/);

  // The URL is the state: a reload keeps the page.
  await page.reload();
  await expect(page.getByText("Showing 26 to 50 of 500")).toBeVisible();

  await page.getByLabel("State").selectOption("stale");
  await expect(page.getByText("Showing 1 to 5 of 5")).toBeVisible();
  await page.getByRole("row", { name: /n099/ }).click();
  await expect(page.getByRole("complementary", { name: "Details" }).getByRole("heading", { name: "n099" })).toBeVisible();

  await page.getByRole("button", { name: "Drain all matching…", exact: true }).click();
  const dlg = page.getByRole("dialog");
  await expect(dlg).toContainText("Drain 5 nodes?");
  expect(drained).toEqual([]);
  await dlg.getByRole("button", { name: "Drain 5" }).click();
  await expect(page.getByRole("status").filter({ hasText: "drain: 5 nodes done" }).first()).toBeVisible();
  expect(drained.sort()).toEqual(["n099", "n199", "n299", "n399", "n499"]);
});
