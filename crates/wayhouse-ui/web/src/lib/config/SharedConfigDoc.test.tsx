import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Link, Route, Routes } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({ getCurrentConfig: vi.fn(), getConfigSchema: vi.fn(), submitConfig: vi.fn(), validateConfig: vi.fn() }));
vi.mock("../../api", async (orig) => ({ ...(await orig<typeof import("../../api")>()), ...api }));
import { SharedConfigDoc, useSharedConfigDoc } from "./SharedConfigDoc";

const FILE = "settings:\n  workers: 2\n";

function Settings() {
  const doc = useSharedConfigDoc();
  return (
    <>
      <button onClick={() => doc.setValue({ settings: { workers: 4 } })}>edit</button>
      <output>{doc.changes} changed</output>
      <Link to="/dashboard">away</Link>
    </>
  );
}
const harness = () =>
  render(
    <MemoryRouter initialEntries={["/settings"]}>
      <Routes>
        <Route element={<SharedConfigDoc />}>
          <Route path="/settings" element={<Settings />} />
          <Route path="/dashboard" element={<Link to="/settings">back</Link>} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );

beforeEach(() => {
  vi.resetAllMocks();
  api.getCurrentConfig.mockResolvedValue({ text: FILE, revision: "7" });
  api.getConfigSchema.mockResolvedValue({ type: "object" });
});

describe("SharedConfigDoc", () => {
  it("reads the document again when a clean editor is entered, but keeps an unsaved draft", async () => {
    harness();
    await screen.findByText("0 changed");
    expect(api.getCurrentConfig).toHaveBeenCalledTimes(1);

    await userEvent.click(screen.getByText("away"));
    await userEvent.click(screen.getByText("back"));
    await waitFor(() => expect(api.getCurrentConfig).toHaveBeenCalledTimes(2)); // nothing was unsaved: refreshed

    await userEvent.click(await screen.findByText("edit"));
    expect(screen.getByText("1 changed")).toBeInTheDocument();
    await userEvent.click(screen.getByText("away"));
    await userEvent.click(screen.getByText("back"));
    expect(await screen.findByText("1 changed")).toBeInTheDocument(); // the draft survived
    expect(api.getCurrentConfig).toHaveBeenCalledTimes(2); // and was not overwritten by a reload
  });

  it("asks before the tab is closed only while there are unsaved changes", async () => {
    harness();
    const unload = () => {
      const e = new Event("beforeunload", { cancelable: true });
      window.dispatchEvent(e);
      return e.defaultPrevented;
    };
    await screen.findByText("0 changed");
    expect(unload()).toBe(false);
    await userEvent.click(screen.getByText("edit"));
    await act(async () => {});
    expect(unload()).toBe(true);
  });
});
