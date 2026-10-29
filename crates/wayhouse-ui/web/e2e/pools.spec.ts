import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { expect, test } from "@playwright/test";

const SCHEMA = JSON.parse(readFileSync(resolve(process.cwd(), "../../wayhouse-config/schema/config.schema.json"), "utf8"));
const CONFIG = ["# edge", "pools:", "  - name: lobby # main", '    targets: ["10.0.0.5:7777"]', "backend_sources:", "  - name: dns", "    type: dns_srv", "    record: _game._tcp.example.com", ""].join("\n");

test("point a pool at a backend source: the YAML is patched, comments stay", async ({ page }) => {
  const posts: string[] = [];
  await page.route("**/ui/session", (r) => r.fulfill({ status: 200, body: "ok" }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/config/schema", (r) => r.fulfill({ json: SCHEMA }));
  await page.route("**/api/ui/layout", (r) => r.fulfill({ status: 404, body: "" }));
  await page.route("**/api/config/validate", (r) => r.fulfill({ json: { ok: true, schema_version: 3 } }));
  await page.route("**/api/config/pending", (r) => r.fulfill({ json: { pending: null, now_ms: Date.now(), auto_confirm_secs: 60, risks: [] } }));
  await page.route(/\/api\/config(\?.*)?$/, (r) => {
    if (r.request().method() === "POST") {
      posts.push(r.request().postData() ?? "");
      return r.fulfill({ json: { revision: 4 } });
    }
    return r.fulfill({ status: 200, headers: { "x-config-revision": "3" }, body: CONFIG });
  });

  await page.goto("/config/pools");
  await expect(page.getByRole("heading", { name: "lobby" })).toBeVisible();
  await page.getByLabel("Backends come from").selectOption("dns");
  await expect(page.getByText(/\d+ unsaved change/)).toBeVisible();
  await page.getByRole("button", { name: "Apply" }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Apply" }).click();
  await expect(page.getByText("Applied as revision 4.")).toBeVisible();
  expect(posts).toHaveLength(1);
  expect(posts[0]).toContain("# edge");
  expect(posts[0]).toContain("name: lobby # main");
  expect(posts[0]).not.toContain("10.0.0.5:7777");
});
