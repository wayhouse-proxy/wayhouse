import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Role } from "../lib/session";
import { renderConfigPage } from "../test/configPage";

const SCHEMA = JSON.parse(readFileSync(resolve(process.cwd(), "../../wayhouse-config/schema/config.schema.json"), "utf8"));
const api = vi.hoisted(() => ({
  getCurrentConfig: vi.fn(),
  getConfigSchema: vi.fn(),
  getLayoutOverride: vi.fn(),
  getPendingConfig: vi.fn(),
  submitConfig: vi.fn(),
  validateConfig: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));
const live = vi.hoisted(() => ({ instances: [] as any[] }));
vi.mock("../useFleetSocket", () => ({ useFleetSocket: () => ({ instances: live.instances, connected: true }) }));

import { PoolsEditorPage, ResolversEditorPage, SourcesEditorPage } from "./ConfigListPages";

const FILE = `# edge
backend_sources:
  - name: dns
    type: dns_srv
    record: _game._tcp.example.com
pools:
  - name: lobby # main
    targets: ["10.0.0.5:7777", "10.0.0.6:7777"]
  - name: eu
    source: dns
    balancer: weighted
resolvers:
  - name: lookup
    endpoint: http://127.0.0.1:9000
listeners:
  - name: game
    bind: 0.0.0.0:25565
    protocol: tcp
    routes:
      - match: { type: always }
        action: { pool: lobby }
`;

beforeEach(() => {
  vi.resetAllMocks();
  live.instances = [];
  api.getCurrentConfig.mockResolvedValue({ text: FILE, revision: "3" });
  api.getConfigSchema.mockResolvedValue(SCHEMA);
  api.getLayoutOverride.mockResolvedValue(null);
  api.getPendingConfig.mockResolvedValue({ pending: null, now_ms: 1, auto_confirm_secs: 60, risks: [] });
  api.submitConfig.mockResolvedValue({ revision: 4 });
  api.validateConfig.mockResolvedValue({ ok: true, schema_version: 4 });
});

const pools = (role: Role = "admin", path = "/config/pools") => renderConfigPage(path, { "/config/pools": <PoolsEditorPage /> }, role);
const sources = (path = "/config/sources") => renderConfigPage(path, { "/config/sources": <SourcesEditorPage /> }, "admin");
const resolvers = (path = "/config/resolvers") => renderConfigPage(path, { "/config/resolvers": <ResolversEditorPage /> }, "admin");

async function apply() {
  await userEvent.click(screen.getByRole("button", { name: "Apply" }));
  const dlg = await screen.findByRole("dialog");
  await within(dlg).findByText(/controller accepts/);
  await userEvent.click(within(dlg).getByRole("button", { name: "Apply" }));
  return api.submitConfig.mock.calls[0][0] as string;
}

describe("Pools editor", () => {
  it("lists the pools and opens the first one with its backends and fields", async () => {
    pools();
    const list = await screen.findByRole("table", { name: "Pools" });
    expect(within(list).getByText("lobby")).toBeInTheDocument();
    expect(within(list).getByText("from source dns")).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "lobby" })).toBeInTheDocument();
    expect(screen.getByLabelText(/^Backends$/)).toHaveValue("10.0.0.5:7777\n10.0.0.6:7777");
    expect(screen.getByRole("img", { name: /Backend 1: 10.0.0.5:7777, state unknown. Backend 2/ })).toBeInTheDocument();
  });

  it("draws live health per backend, over every instance", async () => {
    live.instances = [
      { pools: [{ name: "lobby", backends: [{ addr: "10.0.0.5:7777", healthy: true }, { addr: "10.0.0.6:7777", healthy: false }] }] },
      { pools: [{ name: "lobby", backends: [{ addr: "10.0.0.5:7777", healthy: false }, { addr: "10.0.0.6:7777", healthy: false }] }] },
    ];
    pools();
    expect(await screen.findByRole("img", { name: /Backend 1: 10.0.0.5:7777, degraded. Backend 2: 10.0.0.6:7777, down/ })).toBeInTheDocument();
  });

  it("a field edit is a patch: comments and the other pool survive", async () => {
    pools();
    const timeout = await screen.findByLabelText(/^Connect timeout/);
    await userEvent.type(timeout, "500");
    const text = await apply();
    expect(text).toMatch(/connect_timeout_ms: 300500|connect_timeout_ms: 500/);
    expect(text).toContain("name: lobby # main");
    expect(text).toContain("# edge");
    expect(text).toContain("name: eu");
  });

  it("renaming the open pool keeps it open", async () => {
    pools("admin", "/config/pools?i=1");
    const name = await screen.findByLabelText(/^Name/);
    await userEvent.clear(name);
    await userEvent.type(name, "eu2");
    expect(await screen.findByRole("heading", { name: "eu2" })).toBeInTheDocument();
  });

  it("weights show for the weighted balancer only, and the list is hidden while a source is chosen", async () => {
    pools("admin", "/config/pools?i=1");
    await screen.findByRole("heading", { name: "eu" });
    expect(await screen.findByRole("button", { name: /Add entry/ })).toBeInTheDocument();
    expect(screen.queryByLabelText(/^Backends$/)).toBeNull();
    expect(screen.getByLabelText("Backends come from")).toHaveValue("dns");
  });

  it("switching the balancer away from weighted removes the weights from the YAML", async () => {
    api.getCurrentConfig.mockResolvedValue({
      text: FILE.replace("    balancer: weighted\n", "    balancer: weighted\n    weights: { \"10.0.0.9:1\": 3 }\n"),
      revision: "3",
    });
    pools("admin", "/config/pools?i=1");
    await userEvent.selectOptions(await screen.findByLabelText(/^Balancer/), "round_robin");
    const text = await apply();
    const eu = text.slice(text.indexOf("name: eu"), text.indexOf("resolvers:"));
    expect(eu).toMatch(/balancer: "?round_robin"?/);
    expect(eu).not.toContain("weights");
  });

  it("choosing a source drops the static list from the YAML", async () => {
    pools();
    await userEvent.selectOptions(await screen.findByLabelText("Backends come from"), "dns");
    const text = await apply();
    const lobby = text.slice(text.indexOf("name: lobby"), text.indexOf("name: eu"));
    expect(lobby).toMatch(/source: "?dns"?/);
    expect(lobby).not.toContain("targets");
  });

  it("Add pool starts a named pool; Remove asks first and says where the pool is used", async () => {
    pools();
    await userEvent.click(await screen.findByRole("button", { name: "Add pool" }));
    expect(await screen.findByRole("heading", { name: "pool-1" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("link", { name: "lobby" }));
    await userEvent.click(screen.getByRole("button", { name: "Remove pool" }));
    const dlg = await screen.findByRole("dialog");
    expect(within(dlg).getByText(/used by listener game route 1/)).toBeInTheDocument();
    expect(api.submitConfig).not.toHaveBeenCalled();
    await userEvent.click(within(dlg).getByRole("button", { name: "Remove" }));
    expect(screen.queryByRole("link", { name: "lobby" })).toBeNull();
  });

  it("is read-only below admin", async () => {
    pools("operator");
    expect(await screen.findByRole("button", { name: "Add pool" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByLabelText("Backends come from")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Remove pool" })).toHaveAttribute("aria-disabled", "true");
  });
});

describe("Backend sources editor", () => {
  it("shows the fields of the source's type only", async () => {
    sources();
    expect(await screen.findByLabelText(/^DNS SRV record/)).toHaveValue("_game._tcp.example.com");
    expect(screen.queryByLabelText(/^Addresses/)).toBeNull();
    expect(screen.queryByLabelText(/^Service/)).toBeNull();
  });

  it("switching the type drops the old type's fields and shows the new ones", async () => {
    sources();
    await userEvent.selectOptions(await screen.findByLabelText("Type"), "consul");
    expect(await screen.findByLabelText(/^Service/)).toBeInTheDocument();
    expect(screen.queryByLabelText(/^DNS SRV record/)).toBeNull();
    const text = await apply();
    expect(text).toMatch(/type: "?consul"?/);
    expect(text).not.toContain("record:");
  });
});

describe("Resolvers editor", () => {
  it("edits the endpoint as a patch", async () => {
    resolvers();
    const endpoint = await screen.findByLabelText(/^Endpoint/);
    await userEvent.clear(endpoint);
    await userEvent.type(endpoint, "http://resolver.internal:9000");
    const text = await apply();
    expect(text).toContain("http://resolver.internal:9000");
    expect(text).toContain("# edge");
  });
});
