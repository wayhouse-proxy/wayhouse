import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";

const api = vi.hoisted(() => ({
  getPlugin: vi.fn(),
  getPluginStatus: vi.fn(),
  putPluginConfig: vi.fn(),
  listPluginSecrets: vi.fn(),
  putPluginSecret: vi.fn(),
  deletePluginSecret: vi.fn(),
  enablePluginWebhook: vi.fn(),
  revokePluginWebhook: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { PluginDetailPage } from "./PluginDetailPage";

const CAPS = {
  triggers: { on_timer: true, on_webhook: true },
  tick_interval_secs: 60,
  log: true,
  state: null,
  http: { hosts: [{ host: "p.example" }] },
  secrets: [{ name: "PANEL_TOKEN", description: "panel API key", hosts: ["p.example"] }],
};
const OTHER = { ...CAPS, http: { hosts: [{ host: "q.example" }] } };
const MANIFEST = {
  name: "bound",
  bindings: [{ option: "panel", kind: "hostname" }],
  options: { type: "object", properties: { panel: { type: "string", title: "Panel host" } } },
};

function detail(extra = {}) {
  return {
    id: "0123456789abcdef",
    name: "pelican-sync",
    sha256: "ab".repeat(32),
    size: 2048,
    approved: CAPS,
    config: { panel: "p.example" },
    enabled: true,
    created_at: 1,
    created_by: null,
    manifest: MANIFEST,
    ...extra,
  };
}

function page() {
  render(
    <MemoryRouter initialEntries={["/plugins/0123456789abcdef"]}>
      <Routes>
        <Route path="/plugins/:id" element={<PluginDetailPage />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  for (const f of Object.values(api)) f.mockReset();
  api.getPlugin.mockResolvedValue(detail());
  api.getPluginStatus.mockResolvedValue({ ticks: 0, last_tick_unix: null, last_ok: null, last_error: null, consecutive_failures: 0, held: null, logs: [] });
  api.listPluginSecrets.mockResolvedValue([
    { slot: "PANEL_TOKEN", description: "panel API key", bound_hosts: ["p.example"], set: false, status: "unset" },
  ]);
  api.putPluginConfig.mockResolvedValue(detail());
  api.putPluginSecret.mockResolvedValue(undefined);
  api.enablePluginWebhook.mockResolvedValue({ token: "whk_secret123", path: "/plugins/0123456789abcdef/hook" });
});

describe("Options tab", () => {
  it("saves an edit that leaves the approval alone", async () => {
    page();
    const field = await screen.findByLabelText(/Panel host/);
    expect(field).toHaveValue("p.example");
    await userEvent.clear(field);
    await userEvent.type(field, "p2.example");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(api.putPluginConfig).toHaveBeenCalledWith("0123456789abcdef", { panel: "p2.example" }, undefined);
  });

  it("shows both sets when the edit moves the approval and sends the new one only after the tick", async () => {
    api.putPluginConfig.mockRejectedValueOnce(new ApiError(409, "the new options change the approved capabilities", { approved: CAPS, effective: OTHER }));
    page();
    const field = await screen.findByLabelText(/Panel host/);
    await userEvent.clear(field);
    await userEvent.type(field, "q.example");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    const dlg = await screen.findByRole("dialog");
    expect(dlg).toHaveTextContent("Makes HTTPS requests to p.example");
    expect(dlg).toHaveTextContent("Makes HTTPS requests to q.example");
    const go = within(dlg).getByRole("button", { name: "Approve and save" });
    await userEvent.click(go);
    expect(api.putPluginConfig).toHaveBeenCalledTimes(1);
    await userEvent.click(within(dlg).getByRole("checkbox", { name: /I approve/i }));
    await userEvent.click(go);
    expect(api.putPluginConfig).toHaveBeenLastCalledWith("0123456789abcdef", { panel: "q.example" }, OTHER);
  });

  it("shows the controller's message for any other refusal", async () => {
    api.putPluginConfig.mockRejectedValue(new ApiError(422, "config.panel: must be a hostname"));
    page();
    await userEvent.type(await screen.findByLabelText(/Panel host/), "!");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(await screen.findByText(/config\.panel: must be a hostname/)).toBeInTheDocument();
  });
});

describe("Secrets tab", () => {
  async function open() {
    page();
    await userEvent.click(await screen.findByRole("tab", { name: "Secrets" }));
  }

  it("lists the slots with their state and sets a value write-only", async () => {
    await open();
    const row = (await screen.findByText("PANEL_TOKEN")).closest("tr")!;
    expect(row).toHaveTextContent("not set");
    expect(row).toHaveTextContent("p.example");
    await userEvent.click(within(row).getByRole("button", { name: "Set PANEL_TOKEN" }));
    const dlg = screen.getByRole("dialog");
    const input = within(dlg).getByLabelText("Value");
    expect(input).toHaveAttribute("type", "password");
    await userEvent.type(input, "0123456789abcdefXYZ");
    await userEvent.click(within(dlg).getByRole("button", { name: "Save secret" }));
    expect(api.putPluginSecret).toHaveBeenCalledWith("0123456789abcdef", "PANEL_TOKEN", "0123456789abcdefXYZ");
    expect(screen.queryByText("0123456789abcdefXYZ")).not.toBeInTheDocument();
  });

  it("shows why a slot is held", async () => {
    api.listPluginSecrets.mockResolvedValue([
      { slot: "PANEL_TOKEN", description: "d", bound_hosts: [], set: true, status: "held: sealed with an unknown key" },
    ]);
    await open();
    expect(await screen.findByText(/held: sealed with an unknown key/)).toBeInTheDocument();
  });

  it("shows the controller's refusal in the dialog", async () => {
    api.putPluginSecret.mockRejectedValue(new ApiError(409, "no secret key configured on this controller"));
    await open();
    await userEvent.click(await screen.findByRole("button", { name: "Set PANEL_TOKEN" }));
    await userEvent.type(screen.getByLabelText("Value"), "0123456789abcdef");
    await userEvent.click(screen.getByRole("button", { name: "Save secret" }));
    expect(await screen.findByText(/no secret key configured/)).toBeInTheDocument();
  });
});

describe("Webhook tab", () => {
  it("shows the token once after enabling", async () => {
    page();
    await userEvent.click(await screen.findByRole("tab", { name: "Webhook" }));
    await userEvent.click(screen.getByRole("button", { name: "Enable webhook" }));
    expect(await screen.findByText("whk_secret123")).toBeInTheDocument();
    expect(screen.getByText("/plugins/0123456789abcdef/hook")).toBeInTheDocument();
  });

  it("is absent when the plugin was not approved for a webhook", async () => {
    api.getPlugin.mockResolvedValue(detail({ approved: { ...CAPS, triggers: { on_timer: true } } }));
    page();
    await screen.findByRole("tab", { name: "Options" });
    expect(screen.queryByRole("tab", { name: "Webhook" })).not.toBeInTheDocument();
  });

  it("rotates and revokes an enabled webhook", async () => {
    api.getPlugin.mockResolvedValue(detail({ webhook: { hash: "x" } }));
    api.revokePluginWebhook.mockResolvedValue(undefined);
    page();
    await userEvent.click(await screen.findByRole("tab", { name: "Webhook" }));
    expect(screen.getByRole("button", { name: "Rotate token" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Revoke webhook" }));
    expect(api.revokePluginWebhook).toHaveBeenCalledWith("0123456789abcdef");
  });
});

describe("Status tab", () => {
  it("shows ticks, the held reason, the last error and the log", async () => {
    api.getPluginStatus.mockResolvedValue({
      ticks: 7,
      last_tick_unix: Math.floor(Date.now() / 1000) - 120,
      last_ok: false,
      last_error: "panel said 502",
      consecutive_failures: 2,
      held: "a secret it needs cannot be read on this node",
      logs: [{ level: "warn", msg: "retrying" }],
    });
    page();
    await userEvent.click(await screen.findByRole("tab", { name: "Status and log" }));
    expect(await screen.findByText(/panel said 502/)).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("a secret it needs cannot be read on this node");
    expect(screen.getByText("retrying")).toBeInTheDocument();
    expect(screen.getByText(/2 min ago, failed/)).toBeInTheDocument();
  });
});

it("shows the controller's message for an unknown install", async () => {
  api.getPlugin.mockRejectedValue(new ApiError(404, "no such install"));
  page();
  expect(await screen.findByText(/no such install/)).toBeInTheDocument();
});
