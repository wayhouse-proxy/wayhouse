import { readFileSync } from "node:fs";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
const api = vi.hoisted(() => ({ putClusterSecret: vi.fn(), listClusterSecrets: vi.fn() }));
vi.mock("../../api", async (orig) => ({ ...(await orig<typeof import("../../api")>()), ...api }));

import { SchemaForm } from "./SchemaForm";
import { DEFAULT_LAYOUT, mergeLayout } from "../../lib/form/layout";
import { parseText, patchText } from "../../lib/config/patch";

const SCHEMA = JSON.parse(readFileSync(`${import.meta.dirname}/../../../../../wayhouse-config/schema/config.schema.json`, "utf8"));

const FILE = `# edge
settings:
  workers: 2 # tasks
  admin:
    listen: "127.0.0.1:9901"
pools:
  - name: eu
    targets: ["10.0.0.1:25565"]
listeners:
  - name: mc
    bind: "0.0.0.0:25565"
    pool: eu
`;

/** The form wired like the config page will be: YAML text is the truth, edits are patches. */
function Harness({ path, showAdvanced, layout, page, initial = FILE }: { path?: (string | number)[]; showAdvanced?: boolean; layout?: typeof DEFAULT_LAYOUT; page?: string; initial?: string }) {
  const [text, setText] = useState(initial);
  const value = parseText(text).value;
  const saved = parseText(initial).value;
  return (
    <>
      <SchemaForm schema={SCHEMA} value={value} saved={saved} layout={layout} path={path} showAdvanced={showAdvanced} page={page}
        onChange={(next) => setText((t) => patchText(t, parseText(t).value, next))} />
      <pre data-testid="yaml">{text}</pre>
    </>
  );
}
const yaml = () => screen.getByTestId("yaml").textContent!;

describe("SchemaForm on the real schema", () => {
  it("groups settings by the layout and labels fields in the operator's words", () => {
    render(<Harness path={["settings"]} />);
    for (const g of ["General", "Admin API", "Limits"]) expect(screen.getByRole("button", { name: new RegExp(`^${g}`) })).toBeInTheDocument();
    expect(screen.getByLabelText(/Worker tasks per listener/)).toHaveValue(2);
    expect(screen.getByText("workers")).toBeInTheDocument(); // the YAML key follows the label
  });

  it("a number shows its unit, taken from the layout or the field name", () => {
    render(<Harness path={["settings"]} />);
    expect(screen.getByText("seconds")).toBeInTheDocument(); // shutdown_grace_sec
    expect(screen.getByText("tasks")).toBeInTheDocument();
  });

  it("editing writes a patch: the comment and everything else stay in the file", async () => {
    render(<Harness path={["settings"]} />);
    const workers = screen.getByLabelText(/Worker tasks per listener/);
    await userEvent.tripleClick(workers);
    await userEvent.keyboard("6");
    expect(yaml()).toContain("workers: 6 # tasks");
    expect(yaml()).toContain("# edge");
    expect(yaml()).toContain("listen:");
  });

  it("marks a changed field and counts it on its section", async () => {
    render(<Harness path={["settings"]} />);
    await userEvent.type(screen.getByLabelText(/Worker tasks per listener/), "1");
    expect(screen.getByText("(changed)")).toBeInTheDocument();
    expect(screen.getByText("1 changed")).toBeInTheDocument();
  });

  it("a number cleared to type another keeps its key and comment; leaving it empty deletes the key", async () => {
    render(<Harness path={["settings"]} />);
    const workers = screen.getByLabelText(/Worker tasks per listener/);
    await userEvent.clear(workers);
    expect(yaml()).toContain("workers: 2 # tasks"); // still the saved value while the field is being retyped
    await userEvent.type(workers, "6");
    expect(yaml()).toContain("workers: 6 # tasks"); // the comment survived the retype
    await userEvent.clear(workers);
    await userEvent.tab();
    expect(yaml()).not.toContain("workers");
  });

  it("typing a lone minus sign does not delete a number's key", async () => {
    render(<Harness path={["settings"]} />);
    const workers = screen.getByLabelText(/Worker tasks per listener/);
    await userEvent.clear(workers);
    await userEvent.type(workers, "-");
    expect(yaml()).toContain("workers: 2 # tasks");
  });

  it("blurring after an unparsable draft restores the last value instead of deleting the key", async () => {
    render(<Harness path={["settings"]} />);
    const workers = screen.getByLabelText(/Worker tasks per listener/) as HTMLInputElement;
    await userEvent.clear(workers);
    await userEvent.type(workers, "-");
    // A browser reports value "" and badInput for a lone minus; jsdom does not model that, so say it.
    Object.defineProperty(workers, "validity", { value: { badInput: true }, configurable: true });
    await userEvent.tab();
    expect(yaml()).toContain("workers: 2 # tasks");
    expect(workers).toHaveValue(2);
  });

  it("a { secret } reference is shown read-only and typing elsewhere leaves it in the YAML", async () => {
    const initial = FILE.replace('    listen: "127.0.0.1:9901"\n', '    listen: "127.0.0.1:9901"\n    auth_token: { secret: admin-token }\n');
    render(<Harness path={["settings"]} initial={initial} showAdvanced />);
    const token = screen.getByDisplayValue("secret: admin-token");
    expect(token).toHaveAttribute("readonly");
    await userEvent.type(token, "x");
    expect(yaml()).toMatch(/auth_token: \{ ?secret: admin-token ?\}/);
    expect(yaml()).not.toContain("[object Object]");
    const workers = screen.getByLabelText(/Worker tasks per listener/);
    await userEvent.clear(workers);
    await userEvent.type(workers, "3");
    expect(yaml()).toContain("workers: 3");
    expect(yaml()).toMatch(/auth_token: \{ ?secret: admin-token ?\}/);
  });

  it("blurring an emptied number field still removes the key", async () => {
    render(<Harness path={["settings"]} />);
    const workers = screen.getByLabelText(/Worker tasks per listener/);
    await userEvent.clear(workers);
    await userEvent.tab();
    expect(yaml()).not.toContain("workers:");
  });

  it("every aria-describedby points at text that exists, and a field with no help has none", () => {
    const { container } = render(<Harness path={["settings"]} showAdvanced />);
    const described = [...container.querySelectorAll("[aria-describedby]")];
    expect(described.length).toBeGreaterThan(0);
    for (const el of described) for (const id of el.getAttribute("aria-describedby")!.split(" ")) expect(container.querySelector(`[id="${id}"]`), id).not.toBeNull();
    expect(container.querySelector('[aria-describedby*="-error"]')).toBeNull();
  });

  it("sets machine data in mono and ordinary text in the body face", () => {
    const { container } = render(<Harness path={["settings"]} />);
    const mono = (suffix: string) => container.querySelector<HTMLInputElement>(`input[id$="${suffix}"]`)!.className.includes("font-mono");
    expect(mono("admin_listen")).toBe(true); // an address
    expect(container.querySelector('input[id$="admin_listen"]')).not.toBeNull();
    const { container: pools } = render(<Harness path={["pools", 0]} />);
    expect(pools.querySelector<HTMLInputElement>('input[id$="_name"]')!.className).not.toContain("font-mono"); // a name is a label
  });

  it("hides advanced fields until Show advanced is on", () => {
    const { unmount } = render(<Harness path={["settings"]} />);
    expect(screen.queryByText(/GeoIP database/)).not.toBeInTheDocument();
    unmount();
    render(<Harness path={["settings"]} showAdvanced />);
    expect(screen.getByLabelText(/GeoIP database/)).toBeInTheDocument();
  });

  it("a secret field is a password input", () => {
    render(<Harness path={["settings"]} initial={FILE.replace("listen:", "auth_token: s3cret\n    listen:")} />);
    expect(screen.getByLabelText(/Admin API token/)).toHaveAttribute("type", "password");
  });

  describe("moving a plaintext secret to the secret store", () => {
    const plain = FILE.replace("listen:", "auth_token: s3cret-value\n    listen:");
    beforeEach(() => {
      api.putClusterSecret.mockReset();
      api.putClusterSecret.mockResolvedValue(undefined);
      api.listClusterSecrets.mockReset();
      api.listClusterSecrets.mockResolvedValue([]);
    });

    it("says when the name is taken and what replacing it means, before anything is sent", async () => {
      api.listClusterSecrets.mockResolvedValue([{ name: "auth-token", key_id: "k1", set_at: 1 }]);
      render(<Harness path={["settings"]} initial={plain} />);
      await userEvent.click(screen.getByRole("button", { name: "Move to secret store" }));
      expect(await screen.findByText(/auth-token.* exists/)).toBeInTheDocument();
      expect(api.putClusterSecret).not.toHaveBeenCalled();
      await userEvent.click(screen.getByRole("button", { name: "Replace the stored secret" }));
      expect(api.putClusterSecret).toHaveBeenCalledWith("auth-token", "s3cret-value");
    });

    it("stores the value and leaves only a reference in the YAML", async () => {
      render(<Harness path={["settings"]} initial={plain} />);
      await userEvent.click(screen.getByRole("button", { name: "Move to secret store" }));
      const name = screen.getByLabelText("Secret name");
      expect(name).toHaveValue("auth-token");
      await userEvent.click(screen.getByRole("button", { name: "Store and replace" }));
      expect(api.putClusterSecret).toHaveBeenCalledWith("auth-token", "s3cret-value");
      expect(yaml()).toMatch(/auth_token:\s*(\{ ?secret: auth-token ?\}|\n\s+secret: auth-token)/);
      expect(yaml()).not.toContain("s3cret-value");
    });

    it("keeps the plaintext when the store refuses", async () => {
      api.putClusterSecret.mockRejectedValue(new Error("no key"));
      render(<Harness path={["settings"]} initial={plain} />);
      await userEvent.click(screen.getByRole("button", { name: "Move to secret store" }));
      await userEvent.click(screen.getByRole("button", { name: "Store and replace" }));
      expect(await screen.findByText(/no key/)).toBeInTheDocument();
      expect(yaml()).toContain("auth_token: s3cret-value");
    });

    it("offers nothing for an empty field", () => {
      render(<Harness path={["settings"]} />);
      expect(screen.queryByRole("button", { name: "Move to secret store" })).toBeNull();
    });
  });

  it("an enum is a select, and choosing sets it", async () => {
    render(<Harness path={["pools"]} />);
    const balancer = screen.getByLabelText(/^Balancer/);
    expect(balancer.tagName).toBe("SELECT");
    await userEvent.selectOptions(balancer, "least_conn");
    expect(yaml()).toContain("balancer: least_conn");
  });

  it("a list of strings is one per line", async () => {
    render(<Harness path={["pools"]} />);
    const targets = screen.getByLabelText(/^Backends/);
    expect(targets).toHaveValue("10.0.0.1:25565");
    await userEvent.type(targets, "{Enter}10.0.0.2:25565");
    expect(parseText(yaml()).value).toMatchObject({ pools: [{ targets: ["10.0.0.1:25565", "10.0.0.2:25565"] }] });
  });

  it("adds and removes list items", async () => {
    render(<Harness path={["listeners"]} />);
    expect(screen.getAllByRole("group")).toHaveLength(1);
    await userEvent.click(screen.getByRole("button", { name: /Add listener/ }));
    expect(screen.getAllByRole("group")).toHaveLength(2);
    await userEvent.click(within(screen.getByRole("group", { name: "listener 2" })).getByRole("button", { name: /Remove listener 2/ }));
    expect(screen.getAllByRole("group")).toHaveLength(1);
    expect(yaml()).toContain("name: mc");
  });

  it("a layout override reorders and relabels; the YAML is untouched", () => {
    const layout = mergeLayout(DEFAULT_LAYOUT, { version: 1, fields: { "settings.workers": { label: "Threads" } } });
    render(<Harness path={["settings"]} layout={layout} />);
    expect(screen.getByLabelText(/Threads/)).toBeInTheDocument();
  });

  it("a page shows only the fields the layout puts on it", () => {
    render(<Harness path={["settings"]} page="global" />);
    expect(screen.getByLabelText(/Worker tasks per listener/)).toBeInTheDocument();
    render(<Harness path={["settings"]} page="nowhere" />);
    expect(screen.getAllByLabelText(/Worker tasks per listener/)).toHaveLength(1);
  });

  it("says so when the schema has no such part", () => {
    render(<Harness path={["nope"]} />);
    expect(screen.getByRole("alert")).toHaveTextContent("no part at nope");
  });

  it("a oneOf of string variants (balancer, proxy_protocol) renders as a select with every variant", async () => {
    render(<Harness path={["pools", 0]} showAdvanced />);
    const sel = await screen.findByLabelText(/balancer/i);
    const options = within(sel).getAllByRole("option").map((o) => o.getAttribute("value"));
    expect(options).toEqual(expect.arrayContaining(["round_robin", "least_conn", "consistent_hash", "weighted"]));
    await userEvent.selectOptions(sel, "least_conn");
    expect(yaml()).toMatch(/balancer: least_conn/);
    expect(yaml()).toContain("# edge");
  });

  it("a map (pool weights) gets entries added, renamed and removed", async () => {
    render(<Harness path={["pools", 0]} showAdvanced initial={FILE.replace("pools:\n  - name: eu", "pools:\n  - name: eu\n    balancer: weighted\n    weights:\n      10.0.0.1:25565: 3")} />);
    const key = screen.getByLabelText("Key of entry 10.0.0.1:25565");
    await userEvent.clear(key);
    await userEvent.type(key, "10.0.0.2:25565");
    await userEvent.tab();
    expect(yaml()).toMatch(/10\.0\.0\.2:25565: 3/);
    await userEvent.click(screen.getByRole("button", { name: /Add entry/ }));
    expect(yaml()).toMatch(/newKey: 1\b/);
    await userEvent.click(screen.getByRole("button", { name: "Remove 10.0.0.2:25565" }));
    expect(yaml()).not.toMatch(/10\.0\.0\.2:25565/);
  });

  it("says in words which fields are required", () => {
    render(<Harness path={["pools", 0]} />);
    expect(screen.getAllByText("(required)").length).toBeGreaterThan(0);
  });
});
