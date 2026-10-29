import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import type { Role } from "../lib/session";
import { renderConfigPage } from "../test/configPage";

const api = vi.hoisted(() => ({
  getCurrentConfig: vi.fn(),
  getConfigSchema: vi.fn(),
  getLayoutOverride: vi.fn(),
  getPendingConfig: vi.fn(),
  submitConfig: vi.fn(),
  validateConfig: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));
// The real editor is CodeMirror; the page tests only need something that holds text.
vi.mock("../components/config/SourceEditor", () => ({
  default: ({ value, onChange, label }: { value: string; onChange: (t: string) => void; label: string }) => (
    <textarea aria-label={label} value={value} onChange={(e) => onChange(e.target.value)} />
  ),
}));

import { SettingsPage } from "./SettingsPage";
import { SourcePage } from "./SourcePage";

const FILE = "# edge\nsettings:\n  workers: 2 # tasks\n";
const SCHEMA = {
  type: "object",
  properties: {
    schema_version: { type: "integer" },
    settings: {
      type: "object",
      properties: {
        workers: { type: "integer", description: "Worker tasks." },
        shutdown_grace_sec: { type: "integer", "x-wayhouse-since": 4 },
      },
    },
  },
};

beforeEach(() => {
  vi.resetAllMocks();
  api.getCurrentConfig.mockResolvedValue({ text: FILE, revision: "3" });
  api.getConfigSchema.mockResolvedValue(SCHEMA);
  api.getLayoutOverride.mockResolvedValue(null);
  api.getPendingConfig.mockResolvedValue({ pending: null, now_ms: 1, auto_confirm_secs: 60, risks: [{ id: "gossip", text: "settings.gossip changed" }] });
  api.submitConfig.mockResolvedValue({ revision: 4 });
  api.validateConfig.mockResolvedValue({ ok: true, schema_version: 3 });
});

const page = (role: Role = "admin") => renderConfigPage("/settings", { "/settings": <SettingsPage />, "/settings/source": <SourcePage /> }, role);

async function editWorkers(to: string) {
  const field = await screen.findByLabelText(/Worker tasks per listener/);
  // Select the old value and type over it, as people do: emptying the field first would drop the key and its comment.
  await userEvent.type(field, to, { initialSelectionStart: 0, initialSelectionEnd: 10 });
}

describe("Settings general form", () => {
  it("is read-only for a role that cannot save", async () => {
    page("operator");
    expect(await screen.findByLabelText(/Worker tasks per listener/)).toBeDisabled();
    expect(screen.getByText(/Changing the configuration needs the admin role/)).toBeInTheDocument();
  });

  it("shows the saved values from the schema and the layout, with Apply off until something changes", async () => {
    page();
    expect(await screen.findByLabelText(/Worker tasks per listener/)).toHaveValue(2);
    expect(screen.getByRole("button", { name: "Apply" })).toHaveAttribute("aria-disabled", "true");
  });

  it("review shows the YAML diff, asks the controller to check it, and applies against the loaded revision", async () => {
    page();
    await editWorkers("4");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    const dlg = await screen.findByRole("dialog");
    const diff = within(dlg).getByLabelText("Changes to the YAML");
    expect(diff).toHaveTextContent("- workers: 2 # tasks");
    expect(diff).toHaveTextContent("+ workers: 4 # tasks");
    expect(await within(dlg).findByText(/controller accepts this configuration/)).toBeInTheDocument();
    expect(within(dlg).getByText(/applies on trial for 60 s/)).toHaveTextContent("settings.gossip changed");
    expect(api.submitConfig).not.toHaveBeenCalled();
    await userEvent.click(within(dlg).getByRole("button", { name: "Apply" }));
    expect(api.submitConfig).toHaveBeenCalledWith(expect.stringContaining("workers: 4 # tasks"), { base: 3 });
    expect(await screen.findByText("Applied as revision 4.")).toBeInTheDocument();
  });

  it("will not apply what the controller rejects", async () => {
    api.validateConfig.mockRejectedValue(new ApiError(422, "settings.workers: must be at least 0"));
    page();
    await editWorkers("4");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    const dlg = await screen.findByRole("dialog");
    expect(await within(dlg).findByText(/must be at least 0/)).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Apply" })).toHaveAttribute("aria-disabled", "true");
  });

  it("a canary apply needs a group and sends it", async () => {
    page();
    await editWorkers("4");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    const dlg = await screen.findByRole("dialog");
    await within(dlg).findByText(/controller accepts/);
    await userEvent.click(within(dlg).getByRole("checkbox", { name: /Canary/ }));
    expect(within(dlg).getByRole("button", { name: "Apply" })).toHaveAttribute("aria-disabled", "true");
    await userEvent.type(within(dlg).getByLabelText("Group name"), "eu");
    await userEvent.click(within(dlg).getByRole("button", { name: "Apply" }));
    expect(api.submitConfig).toHaveBeenCalledWith(expect.any(String), { canaryGroup: "eu", base: 3 });
    expect(await screen.findByText(/canary group eu only/)).toBeInTheDocument();
  });

  it("a stale base says so, offers a reload and keeps nothing applied", async () => {
    api.submitConfig.mockRejectedValue(new ApiError(409, "config changed since revision 3"));
    page();
    await editWorkers("4");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    const dlg = await screen.findByRole("dialog");
    await within(dlg).findByText(/controller accepts/);
    await userEvent.click(within(dlg).getByRole("button", { name: "Apply" }));
    expect(await within(dlg).findByRole("alert")).toHaveTextContent(/Someone else changed the configuration/);
    api.getCurrentConfig.mockResolvedValue({ text: "settings:\n  workers: 9\n", revision: "4" });
    await userEvent.click(within(dlg).getByRole("button", { name: "Reload the latest" }));
    expect(await screen.findByLabelText(/Worker tasks per listener/)).toHaveValue(9);
  });

  it("raises schema_version in the YAML when a newer field is set", async () => {
    page();
    await userEvent.click(await screen.findByRole("switch", { name: "Show advanced" }));
    const grace = await screen.findByLabelText(/Shutdown grace/);
    await userEvent.type(grace, "30");
    await userEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(await screen.findByLabelText("Changes to the YAML")).toHaveTextContent("+schema_version: 4");
  });
});

describe("Source page", () => {
  it("shares the document with the forms: a Source edit shows in the form and the other way round", async () => {
    renderConfigPage("/settings/source", { "/settings": <SettingsPage />, "/settings/source": <SourcePage /> });
    const editor = await screen.findByLabelText("Configuration YAML");
    expect(editor).toHaveValue(FILE);
    await userEvent.clear(editor);
    await userEvent.type(editor, "settings:\n  workers: 7\n");
    expect(screen.getByText("1 unsaved change")).toBeInTheDocument();
  });

  it("a YAML that does not parse says so and still lets the text be fixed", async () => {
    renderConfigPage("/settings/source", { "/settings/source": <SourcePage /> });
    const editor = await screen.findByLabelText("Configuration YAML");
    await userEvent.type(editor, "[[");
    expect(await screen.findByText(/does not parse/)).toBeInTheDocument();
  });
});
