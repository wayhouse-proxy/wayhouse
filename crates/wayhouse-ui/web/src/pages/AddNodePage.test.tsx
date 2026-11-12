import { render as rtlRender, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "../api";
import { SessionProvider } from "../lib/session";

const api = vi.hoisted(() => ({
  listEnrollments: vi.fn(),
  mintEnrollment: vi.fn(),
  revokeEnrollment: vi.fn(),
}));
vi.mock("../api", async (orig) => ({ ...(await orig<typeof import("../api")>()), ...api }));

import { AddNodePage } from "./AddNodePage";

const NOW = 1_800_000_000;
const CODE = "k7f2a9.0123456789abcdefghijklmnopqrstuv";
const PIN = `sha256:${"cd".repeat(32)}`;
const URL_ = "https://ctl.example:9901";

function render(path = "/nodes/add", role: "admin" | "viewer" = "admin") {
  return rtlRender(
    <SessionProvider value={{ username: "ana", role }}>
      <MemoryRouter initialEntries={[path]}>
        <AddNodePage />
      </MemoryRouter>
    </SessionProvider>,
  );
}

const row = (id: string, over: object = {}) => ({
  id,
  role: "agent",
  origin: `node-${id}`,
  created_at: NOW - 60,
  expires_at: NOW + 840,
  uses: 1,
  uses_left: 1,
  status: "pending",
  created_by: "ana",
  ...over,
});

async function fillCommon(name = "home-nas") {
  await userEvent.type(screen.getByLabelText(/^Name(?!space)/), name);
  await userEvent.type(screen.getByLabelText(/^Controller URL/), URL_);
  await userEvent.type(screen.getByLabelText(/^Certificate fingerprint/), PIN);
}

beforeEach(() => {
  vi.spyOn(Date, "now").mockReturnValue(NOW * 1000);
  localStorage.clear();
  sessionStorage.clear();
  api.listEnrollments.mockReset().mockResolvedValue({ items: [] });
  api.revokeEnrollment.mockReset().mockResolvedValue(undefined);
  api.mintEnrollment.mockReset().mockImplementation(async (req) => ({
    id: "k7f2a9",
    code: CODE,
    role: req.role,
    name: req.name,
    expires_at: NOW + req.ttl_secs,
    uses: req.uses,
  }));
});

describe("AddNodePage form", () => {
  it("needs the admin role", async () => {
    render("/nodes/add", "viewer");
    expect(await screen.findByText("Adding a node needs the admin role.")).toBeInTheDocument();
    expect(api.listEnrollments).not.toHaveBeenCalled();
  });

  it("pre-fills role and name from the query, which is how Re-enroll arrives", async () => {
    render("/nodes/add?role=aggregator&name=agg-1");
    expect(await screen.findByLabelText(/^Role/)).toHaveValue("aggregator");
    expect(screen.getByLabelText(/^Name(?!space)/)).toHaveValue("agg-1");
  });

  it("will not mint with a bad name, URL or fingerprint, and says which", async () => {
    render();
    await userEvent.type(screen.getByLabelText(/^Name(?!space)/), "bad name");
    await userEvent.type(screen.getByLabelText(/^Controller URL/), "http://u:p@x");
    await userEvent.type(screen.getByLabelText(/^Certificate fingerprint/), "sha256:12");
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    expect(api.mintEnrollment).not.toHaveBeenCalled();
    expect(await screen.findByText(/1 to 64 letters/)).toBeInTheDocument();
    expect(screen.getByText(/An https:\/\/ URL/)).toBeInTheDocument();
    expect(screen.getByText(/sha256: followed by 64 hex/)).toBeInTheDocument();
  });

  it("offers the controller replica role only as a disabled placeholder", () => {
    render();
    const opt = screen.getByRole("option", { name: /Controller replica/ });
    expect(opt).toBeDisabled();
  });

  it("explains that an agent cannot go to Kubernetes yet and refuses to mint it", async () => {
    render();
    await userEvent.selectOptions(screen.getByLabelText(/^Set up with/), "k8s");
    expect(screen.getByText(/needs the per-pod init container/)).toBeInTheDocument();
    await fillCommon();
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    expect(api.mintEnrollment).not.toHaveBeenCalled();
  });

  it("limits a keyring role to 24 hours and a single-use UI code to one use", async () => {
    render();
    await userEvent.selectOptions(screen.getByLabelText(/^Role/), "proxy");
    expect(screen.queryByRole("option", { name: "7 days" })).not.toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText(/^Role/), "ui");
    expect(screen.getByLabelText(/^Uses/)).toBeDisabled();
    expect(screen.getByLabelText(/^Uses/)).toHaveValue(1);
  });
});

describe("AddNodePage mint", () => {
  it("mints an agent code and shows the command with the code once", async () => {
    render();
    await fillCommon();
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    expect(api.mintEnrollment).toHaveBeenCalledWith({ role: "agent", name: "home-nas", ttl_secs: 900, uses: 1 });
    const step = await screen.findByLabelText("Step 1 text");
    expect(step.textContent).toContain(`WAYHOUSE_ENROLL_CODE=${CODE}`);
    expect(step.textContent).toContain("wayhouse-agent install --systemd");
    expect(screen.getByText(/shown once/)).toBeInTheDocument();
    expect(screen.getByText(/works one time/)).toBeInTheDocument();
  });

  it("never writes the code to browser storage, only the public settings", async () => {
    render();
    await fillCommon();
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    await screen.findByLabelText("Step 1 text");
    const stored = JSON.stringify({ ...localStorage }) + JSON.stringify({ ...sessionStorage });
    expect(stored).not.toContain(CODE);
    expect(stored).not.toContain("0123456789abcdef");
    expect(JSON.parse(localStorage.getItem("wayhouse.addnode.prefs")!)).toMatchObject({ controllerUrl: URL_, pin: PIN });
  });

  it("hides the code for good after Done", async () => {
    render();
    await fillCommon();
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    await screen.findByLabelText("Step 1 text");
    await userEvent.click(screen.getByRole("button", { name: "Done, hide the code" }));
    expect(screen.queryByLabelText("Step 1 text")).not.toBeInTheDocument();
    expect(document.body.textContent).not.toContain(CODE);
    expect(screen.getByRole("form", { name: "New node" })).toBeInTheDocument();
  });

  it("the prompt form keeps the code out of the command and shows it apart", async () => {
    render();
    await fillCommon();
    await userEvent.click(screen.getByLabelText(/^Keep the code out of the command/));
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    const step = await screen.findByLabelText("Step 1 text");
    expect(step.textContent).not.toContain(CODE);
    expect(screen.getByLabelText("Enrollment code")).toHaveValue(CODE);
  });

  it("builds the Compose variant from the chosen target", async () => {
    render();
    await userEvent.selectOptions(screen.getByLabelText(/^Set up with/), "compose");
    await fillCommon();
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    expect(await screen.findByRole("heading", { name: /compose\.home-nas\.yml/ })).toBeInTheDocument();
    expect(screen.getByLabelText("Step 2 text").textContent).toContain("WAYHOUSE_ENROLL_CODE_FILE");
  });

  it("copies a step to the clipboard", async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText: write }, configurable: true });
    render();
    await fillCommon();
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    await userEvent.click(await screen.findByRole("button", { name: "Copy step 1" }));
    expect(write).toHaveBeenCalledWith(expect.stringContaining("wayhouse-agent install --systemd"));
    expect(await screen.findByText("Copied")).toBeInTheDocument();
  });

  it("tells the operator when the browser refuses to copy", async () => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: vi.fn().mockRejectedValue(new Error("no")) }, configurable: true });
    render();
    await fillCommon();
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    await userEvent.click(await screen.findByRole("button", { name: "Copy step 1" }));
    expect(await screen.findByText(/refused to copy/)).toBeInTheDocument();
  });

  it("shows the controller's refusal and stays on the form", async () => {
    api.mintEnrollment.mockRejectedValue(new ApiError(409, "this controller has no secret key"));
    render();
    await fillCommon();
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    expect(await screen.findByText("this controller has no secret key")).toBeInTheDocument();
    expect(screen.queryByLabelText("Step 1 text")).not.toBeInTheDocument();
  });
});

describe("AddNodePage step-up", () => {
  async function proxyForm() {
    render();
    await userEvent.selectOptions(screen.getByLabelText(/^Role/), "proxy");
    await fillCommon("edge-1");
  }

  it("asks for the secret, sends it with the mint, and empties the field", async () => {
    await proxyForm();
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    expect(api.mintEnrollment).not.toHaveBeenCalled();
    expect(await screen.findByText("Required for this role.")).toBeInTheDocument();

    const field = screen.getByLabelText(/^Step-up secret/);
    expect(field).toHaveAttribute("type", "password");
    await userEvent.type(field, "a long enough step-up secret");
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    expect(api.mintEnrollment).toHaveBeenCalledWith({
      role: "proxy",
      name: "edge-1",
      ttl_secs: 900,
      uses: 1,
      step_up: "a long enough step-up secret",
    });
    await screen.findByLabelText("Step 1 text");
    expect(document.body.textContent).not.toContain("a long enough step-up secret");
    expect(JSON.stringify({ ...localStorage })).not.toContain("step-up secret");
  });

  it("shows a wrong secret as an error on the field and does not keep what was typed", async () => {
    api.mintEnrollment.mockRejectedValue(new ApiError(403, "this role needs the step-up secret; enter it again"));
    await proxyForm();
    const field = screen.getByLabelText(/^Step-up secret/);
    await userEvent.type(field, "wrong secret value here");
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    expect(await screen.findByText(/enter it again/)).toBeInTheDocument();
    expect(field).toHaveValue("");
    expect(screen.queryByLabelText("Step 1 text")).not.toBeInTheDocument();
  });

  it("does not send a step-up secret for a role that does not need one", async () => {
    render();
    await fillCommon();
    expect(screen.queryByLabelText(/^Step-up secret/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    expect(api.mintEnrollment.mock.calls[0][0]).not.toHaveProperty("step_up");
  });

  it("sends an aggregator's parent URL", async () => {
    render();
    await userEvent.selectOptions(screen.getByLabelText(/^Role/), "aggregator");
    await fillCommon("agg-1");
    await userEvent.type(screen.getByLabelText(/^Parent aggregator URL/), "https://parent.example:9902");
    await userEvent.click(screen.getByRole("button", { name: "Create code" }));
    expect(api.mintEnrollment).toHaveBeenCalledWith({
      role: "aggregator",
      name: "agg-1",
      ttl_secs: 900,
      uses: 1,
      parent_url: "https://parent.example:9902",
    });
  });
});

describe("AddNodePage codes list", () => {
  it("lists pending, used and expired codes and never a code", async () => {
    api.listEnrollments.mockResolvedValue({
      items: [
        row("a1"),
        row("b2", { status: "used", uses_left: 0, role: "proxy" }),
        row("c3", { status: "expired", expires_at: NOW - 7200 }),
      ],
    });
    render();
    const pending = (await screen.findByText("node-a1")).closest("tr")!;
    expect(within(pending).getByText("pending")).toBeInTheDocument();
    expect(within(pending).getByText("in 14 min")).toBeInTheDocument();
    expect(within(pending).getByText("1 of 1")).toBeInTheDocument();
    const used = screen.getByText("node-b2").closest("tr")!;
    expect(within(used).getByText("used")).toBeInTheDocument();
    expect(within(used).getByText("0 of 1")).toBeInTheDocument();
    const expired = screen.getByText("node-c3").closest("tr")!;
    expect(within(expired).getByText("expired")).toBeInTheDocument();
    expect(within(expired).getByText("2 h ago")).toBeInTheDocument();
  });

  it("revokes after a confirmation and reloads, which removes the row", async () => {
    api.listEnrollments.mockResolvedValueOnce({ items: [row("a1")] }).mockResolvedValue({ items: [] });
    render();
    await userEvent.click(await screen.findByRole("button", { name: "Revoke code for node-a1" }));
    expect(api.revokeEnrollment).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole("button", { name: "Revoke code" }));
    expect(api.revokeEnrollment).toHaveBeenCalledWith("a1");
    expect(await screen.findByText("revoked the code for node-a1")).toBeInTheDocument();
    expect(screen.queryByText("node-a1")).not.toBeInTheDocument();
    expect(screen.getByText(/No codes/)).toBeInTheDocument();
  });

  it("revokes nothing when cancelled", async () => {
    api.listEnrollments.mockResolvedValue({ items: [row("a1")] });
    render();
    await userEvent.click(await screen.findByRole("button", { name: "Revoke code for node-a1" }));
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.revokeEnrollment).not.toHaveBeenCalled();
  });

  it("shows a failed revoke", async () => {
    api.revokeEnrollment.mockRejectedValue(new ApiError(404, "no such code"));
    api.listEnrollments.mockResolvedValue({ items: [row("a1")] });
    render();
    await userEvent.click(await screen.findByRole("button", { name: "Revoke code for node-a1" }));
    await userEvent.click(screen.getByRole("button", { name: "Revoke code" }));
    expect(await screen.findByText("revoke failed: no such code")).toBeInTheDocument();
  });

  it("shows the controller's error instead of a table", async () => {
    api.listEnrollments.mockRejectedValue(new ApiError(503, "no controller configured (--controller-url)"));
    render();
    expect(await screen.findByText("no controller configured (--controller-url)")).toBeInTheDocument();
    expect(screen.queryByRole("table")).not.toBeInTheDocument();
  });
});
