import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FleetNode } from "../../lib/fleet/nodes";

const api = vi.hoisted(() => ({ setNodeTags: vi.fn(), clearNodeTags: vi.fn() }));
vi.mock("../../api", async (orig) => ({ ...(await orig<typeof import("../../api")>()), ...api }));

import { SessionProvider, type Role } from "../../lib/session";
import { ToastProvider } from "../ui/Toast";
import { NodeTags } from "./NodeTags";

const NODE = { instance: "n1", tags: { env: "prod", rack: "r1" }, operator_tags: { rack: "r1" } } as unknown as FleetNode;

function show(props: Partial<React.ComponentProps<typeof NodeTags>> = {}, role: Role = "operator") {
  return render(
    <SessionProvider value={{ username: "ana", role }}>
      <QueryClientProvider client={new QueryClient()}>
        <ToastProvider>
          <NodeTags instance="n1" node={NODE} onTag={() => {}} {...props} />
        </ToastProvider>
      </QueryClientProvider>
    </SessionProvider>,
  );
}

beforeEach(() => {
  vi.resetAllMocks();
  api.setNodeTags.mockResolvedValue(undefined);
  api.clearNodeTags.mockResolvedValue(undefined);
});

describe("node tags", () => {
  it("separates what the node reports from what an operator set", () => {
    show();
    expect(screen.getByRole("button", { name: "env=prod" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Remove tag rack" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove tag env" })).toBeNull();
  });

  it("adds a tag by sending the whole operator set", async () => {
    show();
    await userEvent.type(screen.getByLabelText("Tag key"), "zone");
    await userEvent.type(screen.getByLabelText("Tag value"), "a1");
    await userEvent.click(screen.getByRole("button", { name: "Add tag" }));
    expect(api.setNodeTags.mock.calls[0].slice(0, 2)).toEqual(["n1", { rack: "r1", zone: "a1" }]);
  });

  it("removing the last operator tag clears them", async () => {
    show();
    await userEvent.click(screen.getByRole("button", { name: "Remove tag rack" }));
    expect(api.clearNodeTags.mock.calls[0][0]).toBe("n1");
    expect(api.setNodeTags).not.toHaveBeenCalled();
  });

  it("refuses a bad key without calling the API", async () => {
    show();
    await userEvent.type(screen.getByLabelText("Tag key"), "Bad Key");
    await userEvent.type(screen.getByLabelText("Tag value"), "x");
    await userEvent.click(screen.getByRole("button", { name: "Add tag" }));
    expect(screen.getByText(/lowercase letters/)).toBeInTheDocument();
    expect(api.setNodeTags).not.toHaveBeenCalled();
  });

  it("is read-only for a viewer", () => {
    show({}, "viewer");
    expect(screen.getByLabelText("Tag key")).toBeDisabled();
    expect(screen.getByRole("button", { name: "Remove tag rack" })).toBeDisabled();
  });

  it("says so, and stops editing, when the controller's tags are unavailable", () => {
    show({ unavailable: true });
    expect(screen.getByText(/operator tags are unavailable/i)).toBeInTheDocument();
    expect(screen.getByLabelText("Tag key")).toBeDisabled();
  });

  it("cannot tag an id the controller cannot address", () => {
    show({ instance: "a/b" });
    expect(screen.getByText(/cannot be tagged/i)).toBeInTheDocument();
    expect(screen.getByLabelText("Tag key")).toBeDisabled();
  });
});
