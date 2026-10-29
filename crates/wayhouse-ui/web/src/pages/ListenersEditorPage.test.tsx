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

import { ListenersEditorPage } from "./ListenersEditorPage";

const FILE = `# edge
pools:
  - name: lobby
    targets: ["10.0.0.5:7777"]
  - name: eu
    targets: ["10.0.1.5:7777"]
resolvers:
  - name: lookup
    endpoint: http://127.0.0.1:9000
listeners:
  - name: game # main
    bind: 0.0.0.0:25565
    protocol: tcp
    routes:
      - match: { type: sni, host: [a.example] }
        action: { pool: lobby }
      - match: { type: always }
        action: { pool: eu }
  - name: voice
    bind: 0.0.0.0:30000
    protocol: udp
    pool: lobby
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

const page = (role: Role = "admin", path = "/config/listeners") => renderConfigPage(path, { "/config/listeners": <ListenersEditorPage /> }, role);
const route = (n: number) => screen.findByRole("group", { name: `Route ${n}` });

async function apply() {
  await userEvent.click(screen.getByRole("button", { name: "Apply" }));
  const dlg = await screen.findByRole("dialog");
  await within(dlg).findByText(/controller accepts/);
  await userEvent.click(within(dlg).getByRole("button", { name: "Apply" }));
  const text = api.submitConfig.mock.calls[0][0] as string;
  return text;
}

describe("Listeners and routes editor", () => {
  it("lists the listeners and opens the first, with its fields from the schema and its routes", async () => {
    page();
    const list = await screen.findByRole("table", { name: "Listeners" });
    expect(within(list).getByText("voice")).toBeInTheDocument();
    expect(await screen.findByLabelText(/^Bind address/)).toHaveValue("0.0.0.0:25565");
    expect(within(await route(1)).getByLabelText("Match type of route 1")).toHaveValue("sni");
    expect(within(await route(2)).getByLabelText("Target of route 2")).toHaveValue("eu");
  });

  it("opens the listener named in the URL", async () => {
    page("admin", "/config/listeners?name=voice");
    expect(await screen.findByLabelText(/^Bind address/)).toHaveValue("0.0.0.0:30000");
    expect(screen.getByLabelText(/^Pool/)).toHaveValue("lobby");
  });

  it("a field edit is a patch: comments and the other listener survive", async () => {
    page();
    const bind = await screen.findByLabelText(/^Bind address/);
    await userEvent.type(bind, "0.0.0.0:25570", { initialSelectionStart: 0, initialSelectionEnd: 13 });
    const text = await apply();
    expect(text).toMatch(/bind: "?0\.0\.0\.0:25570"?/);
    expect(text).toContain("name: game # main");
    expect(text).toContain("# edge");
    expect(text).toContain("name: voice");
  });

  it("the match type picker shows only the fields that type takes", async () => {
    page();
    const r1 = await route(1);
    expect(within(r1).getByLabelText(/Host patterns/)).toBeInTheDocument();
    await userEvent.selectOptions(within(r1).getByLabelText("Match type of route 1"), "port");
    expect(within(r1).queryByLabelText(/Host patterns/)).toBeNull();
    expect(within(r1).getByLabelText(/Ports, one per line/)).toBeInTheDocument();
    await userEvent.selectOptions(within(r1).getByLabelText("Match type of route 1"), "first_bytes");
    expect(within(r1).getByLabelText(/Prefix \(hex/)).toBeInTheDocument();
    expect(within(r1).getByLabelText("Length min (route 1)")).toBeInTheDocument();
    await userEvent.selectOptions(within(r1).getByLabelText("Match type of route 1"), "always");
    expect(within(r1).queryByRole("textbox")).toBeNull();
  });

  it("switching a route's type writes a YAML the parser accepts: the old type's fields are gone", async () => {
    page();
    const r1 = await route(1);
    await userEvent.selectOptions(within(r1).getByLabelText("Match type of route 1"), "port");
    await userEvent.type(within(r1).getByLabelText(/Ports, one per line/), "30001{Enter}30010-30020");
    const text = await apply();
    expect(text).not.toContain("a.example");
    expect(text).toMatch(/type: port/);
    expect(text).toMatch(/30001/);
    expect(text).toMatch(/30010-30020/);
  });

  it("moves routes (order matters), adds and removes them", async () => {
    page();
    await route(2);
    await userEvent.click(screen.getByRole("button", { name: "Move route 2 up" }));
    expect(within(await route(1)).getByLabelText("Match type of route 1")).toHaveValue("always");
    expect(screen.getByRole("button", { name: "Move route 1 up" })).toHaveAttribute("aria-disabled", "true");
    await userEvent.click(screen.getByRole("button", { name: "Add route" }));
    expect(within(await route(3)).getByLabelText("Target of route 3")).toHaveValue("lobby");
    await userEvent.click(screen.getByRole("button", { name: "Remove route 1" }));
    expect(screen.queryByRole("group", { name: "Route 3" })).toBeNull();
    const text = await apply();
    // sni route first now? No: "always" moved to the top and was removed, so only the sni route and the new one remain.
    expect(text).toContain("a.example");
    expect(text.match(/pool: lobby/g)!.length).toBeGreaterThanOrEqual(2);
  });

  it("a target that is not defined is flagged, and a resolver can be chosen", async () => {
    api.getCurrentConfig.mockResolvedValue({ text: FILE.replace("pool: eu", "pool: gone"), revision: "3" });
    page();
    const r2 = await route(2);
    expect(within(r2).getByRole("option", { name: "gone (not defined)" })).toBeInTheDocument();
    await userEvent.selectOptions(within(r2).getByLabelText("Target kind of route 2"), "resolver");
    await userEvent.selectOptions(within(r2).getByLabelText("Target of route 2"), "lookup");
    const text = await apply();
    expect(text).toMatch(/resolver: "?lookup"?/);
    expect(text).not.toContain("pool: gone");
  });

  it("adds a listener with a fresh name and selects it", async () => {
    page();
    await screen.findByLabelText(/^Bind address/);
    await userEvent.click(screen.getByRole("button", { name: "Add listener" }));
    expect(await screen.findByRole("heading", { name: "listener-1" })).toBeInTheDocument();
    const text = await apply();
    expect(text).toContain("name: listener-1");
    expect(text).toContain("protocol: tcp");
  });

  it("removing a listener asks first", async () => {
    page();
    await screen.findByLabelText(/^Bind address/);
    await userEvent.click(screen.getByRole("button", { name: "Remove listener" }));
    expect(api.submitConfig).not.toHaveBeenCalled();
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Remove" }));
    expect(await screen.findByRole("heading", { name: "voice" })).toBeInTheDocument();
    expect(screen.getByText(/\d+ unsaved change/)).toBeInTheDocument();
  });

  it("draws the patch lines with live pool state and spells them out", async () => {
    live.instances = [{ instance: "n1", pools: [{ name: "lobby", balancer: "rr", backends: [{ addr: "a", healthy: true, state: "enabled", active: 0 }] }, { name: "eu", balancer: "rr", backends: [{ addr: "b", healthy: false, state: "enabled", active: 0 }] }] }];
    page();
    const drawing = await screen.findByRole("img", { name: /^game / });
    expect(drawing).toHaveAccessibleName(/Route 1: SNI a.example to pool lobby, up\. Route 2: everything to pool eu, down/);
    await userEvent.click(screen.getByText("View as table"));
    const table = screen.getByRole("table", { name: "Routes of game, as a table" });
    expect(within(table).getAllByRole("row")).toHaveLength(3);
  });

  it("a role that cannot save sees everything but changes nothing", async () => {
    page("operator");
    expect(await screen.findByLabelText(/^Bind address/)).toBeDisabled();
    expect(screen.getByRole("button", { name: "Add listener" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("button", { name: "Add route" })).toHaveAttribute("aria-disabled", "true");
    expect(within(await route(1)).getByLabelText("Match type of route 1")).toBeDisabled();
  });
});
