import { expect, test } from "@playwright/test";

const T0 = Date.UTC(2026, 9, 29, 12, 0);
const points = Array.from({ length: 10 }, (_, i) => ({
  t_ms: T0 + i * 60_000,
  partial: i === 9,
  accepted_per_sec: 20 + i,
  blocked_per_sec: i < 5 ? 0 : 2,
  bytes_per_sec: 1000 * (i + 1),
  packets_per_sec: 4,
  tcp_sessions: 30 + i,
  udp_sessions: 5,
}));

test("overview draws real charts and the same data is available as a table", async ({ page }) => {
  await page.route("**/ui/session", (r) =>
    r.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify({ authenticated: true, username: "ana", role: "viewer" }) }),
  );
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/fleet/stats**", (r) =>
    r.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        generated_at_ms: Date.now(),
        range_sec: 900,
        fleet: { resolution_sec: 60, last_sample_ms: T0 + 9 * 60_000, points },
        listeners: [],
        pools: [],
        regions: [],
      }),
    }),
  );
  await page.goto("/overview");
  const chart = page.getByRole("img", { name: /^Connections\./ });
  await expect(chart).toBeVisible();
  await expect(chart.locator("svg path.recharts-line-curve").first()).toBeVisible();
  await expect(chart.locator(".recharts-reference-line")).toHaveCount(1); // blocking began at 12:05
  await page.getByRole("button", { name: "View as table" }).first().click();
  const rows = page.getByRole("table", { name: "Connections, as a table" }).getByRole("row");
  await expect(rows).toHaveCount(11);
});
