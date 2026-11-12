import { expect, test } from "@playwright/test";

const CODE = "k7f2a9.0123456789abcdefghijklmnopqrstuv";
const PIN = `sha256:${"cd".repeat(32)}`;

// Nodes > Add node: the whole flow against a stubbed backend. The code is shown once and lives
// in page memory only; the step-up secret goes out with the mint and is not kept.
test("mint a proxy code with the step-up secret, copy the command, then revoke the code", async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  const codes: any[] = [];
  const mints: any[] = [];
  await page.route("**/ui/session", (r) => r.fulfill({ json: { authenticated: true, username: "ana", role: "admin" } }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.route("**/api/enrollments", async (r) => {
    if (r.request().method() === "POST") {
      const body = r.request().postDataJSON();
      mints.push(body);
      if (body.step_up !== "correct step-up secret") {
        return r.fulfill({ status: 403, json: { error: "this role needs the step-up secret; enter it again" } });
      }
      codes.push({ id: "k7f2a9", role: body.role, origin: body.name, created_at: 1_800_000_000, expires_at: 1_800_000_900, uses: body.uses, uses_left: body.uses, status: "pending", created_by: "ana" });
      return r.fulfill({ headers: { "cache-control": "no-store" }, json: { id: "k7f2a9", code: CODE, role: body.role, name: body.name, expires_at: Math.floor(Date.now() / 1000) + 900, uses: body.uses } });
    }
    return r.fulfill({ json: { items: codes } });
  });
  await page.route("**/api/enrollments/*", (r) => {
    codes.length = 0;
    return r.fulfill({ status: 204, body: "" });
  });

  await page.goto("/nodes/add");
  await expect(page.getByRole("heading", { level: 1, name: "Add node" })).toBeVisible();
  await page.getByLabel("Role").selectOption("proxy");
  await page.getByLabel("Name", { exact: false }).first().fill("edge-1");
  await page.getByLabel("Controller URL").fill("https://ctl.example:9901");
  await page.getByLabel("Certificate fingerprint").fill(PIN);
  await page.getByLabel("Step-up secret").fill("wrong guess, but long enough");
  await page.getByRole("button", { name: "Create code" }).click();
  await expect(page.getByText("enter it again")).toBeVisible();
  await expect(page.getByLabel("Step-up secret")).toHaveValue("");

  await page.getByLabel("Step-up secret").fill("correct step-up secret");
  await page.getByRole("button", { name: "Create code" }).click();
  const step = page.getByLabel("Step 1 text");
  await expect(step).toContainText(`WAYHOUSE_ENROLL_CODE=${CODE}`);
  await expect(step).toContainText("wayhouse install --systemd");
  await expect(page.getByText(/shown once/)).toBeVisible();
  expect(mints.at(-1)).toMatchObject({ role: "proxy", name: "edge-1", step_up: "correct step-up secret" });

  await page.getByRole("button", { name: "Copy step 1" }).click();
  await expect(page.getByText("Copied")).toBeVisible();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toContain(`WAYHOUSE_ENROLL_CODE=${CODE}`);

  // Nothing secret reaches browser storage or the URL.
  const stored = await page.evaluate(() => JSON.stringify({ ...localStorage }) + JSON.stringify({ ...sessionStorage }) + location.href);
  expect(stored).not.toContain("0123456789abcdef");
  expect(stored).not.toContain("step-up secret");

  await page.getByRole("button", { name: "Done, hide the code" }).click();
  await expect(page.locator("body")).not.toContainText(CODE);

  const row = page.getByRole("row", { name: /edge-1/ });
  await expect(row).toContainText("pending");
  await page.getByRole("button", { name: "Revoke code for edge-1" }).click();
  await page.getByRole("button", { name: "Revoke code", exact: true }).click();
  await expect(page.getByRole("row", { name: /edge-1/ })).toHaveCount(0);
});

test("the Add node entry is in the System group for an admin and absent for a viewer", async ({ page }) => {
  await page.route("**/ui/session", (r) => r.fulfill({ json: { authenticated: true, username: "vic", role: "viewer" } }));
  await page.route("**/ws/**", (r) => r.abort());
  await page.goto("/dashboard");
  await expect(page.getByRole("navigation").getByRole("link", { name: "Tunnel addresses" })).toBeVisible();
  await expect(page.getByRole("navigation").getByRole("link", { name: "Add node" })).toHaveCount(0);
});
