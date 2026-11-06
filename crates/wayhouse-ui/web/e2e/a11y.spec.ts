import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

// An automated accessibility pass over every page, in both themes (docs/design-system/accessibility.md).
// It runs on the demo build (`npm run build:demo`, which CI runs before the e2e step): the demo carries
// a mock backend with realistic data, so each page renders its real content, tables and charts included.
// Axe finds what a tool can (names, roles, contrast, landmarks); the keyboard and focus rules in that
// document are covered by the page specs. Serious and critical violations fail the run, and so does a
// page without exactly one level-1 heading.
const DEMO = pathToFileURL(resolve(process.cwd(), "dist-demo/wayhouse-demo.html")).href;
const PAGES = [
  "/dashboard", "/overview", "/fleet", "/listeners", "/pools", "/features", "/plugins", "/plugins/new",
  "/settings", "/settings/appearance", "/settings/secrets", "/sniffers", "/certificates", "/config-history", "/tunnel",
  "/config/listeners", "/config/pools",
];

for (const scheme of ["light", "dark"] as const) {
  test(`no serious accessibility violations in the ${scheme} theme`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: scheme });
    const found: string[] = [];
    const titles = new Set<string>();
    for (const path of PAGES) {
      // Leave the document first: a hash change alone keeps the old page (and its heading) on screen
      // until the router re-renders, so the heading check below could be met by the previous page.
      await page.goto("about:blank");
      await page.goto(`${DEMO}#${path}`);
      await expect(page.getByRole("heading", { level: 1 }).first(), path).toBeVisible();
      await page.waitForTimeout(300); // charts and tables settle
      titles.add((await page.getByRole("heading", { level: 1 }).first().textContent()) ?? "");
      if ((await page.getByRole("heading", { level: 1 }).count()) !== 1) found.push(`${path} needs exactly one level-1 heading`);
      const { violations } = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21a", "wcag21aa"]).analyze();
      for (const v of violations.filter((v) => v.impact === "serious" || v.impact === "critical")) {
        found.push(`${path} ${v.id} (${v.impact}): ${v.nodes.length} node(s), e.g. ${v.nodes[0]?.target.join(" ")}`);
      }
    }
    expect(found).toEqual([]);
    // Guards against a demo build that ignores the route and shows one page 17 times.
    expect(titles.size).toBeGreaterThan(10);
  });
}
