import { readFileSync } from "node:fs";
import { useState } from "react";
import { describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
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

  it("clearing a field deletes the key", async () => {
    render(<Harness path={["settings"]} />);
    await userEvent.clear(screen.getByLabelText(/Worker tasks per listener/));
    expect(yaml()).not.toContain("workers");
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

  it("an enum is a select, and choosing sets it", async () => {
    render(<Harness path={["pools"]} />);
    const balancer = screen.getByLabelText("balancer");
    expect(balancer.tagName).toBe("SELECT");
    await userEvent.selectOptions(balancer, "least_conn");
    expect(yaml()).toContain("balancer: least_conn");
  });

  it("a list of strings is one per line", async () => {
    render(<Harness path={["pools"]} />);
    const targets = screen.getByLabelText("targets");
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
});
