import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook, waitFor } from "@testing-library/react";
import { ApiError } from "../../api";

const api = vi.hoisted(() => ({
  getCurrentConfig: vi.fn(),
  getConfigSchema: vi.fn(),
  submitConfig: vi.fn(),
  validateConfig: vi.fn(),
}));
vi.mock("../../api", async (orig) => ({ ...(await orig<typeof import("../../api")>()), ...api }));
import { useConfigDoc } from "./useConfigDoc";

const FILE = "# edge\nsettings:\n  workers: 2 # tasks\n";

beforeEach(() => {
  vi.resetAllMocks();
  api.getCurrentConfig.mockResolvedValue({ text: FILE, revision: "7" });
  api.getConfigSchema.mockResolvedValue({ type: "object" });
});

async function loaded() {
  const hook = renderHook(() => useConfigDoc());
  await waitFor(() => expect(hook.result.current.loading).toBe(false));
  return hook;
}

describe("useConfigDoc", () => {
  it("loads the text, the revision and the schema", async () => {
    const { result } = await loaded();
    expect(result.current.text).toBe(FILE);
    expect(result.current.revision).toBe(7);
    expect(result.current.schema).toEqual({ type: "object" });
    expect(result.current.changes).toBe(0);
  });

  it("a form edit patches the text, counts as a change and can be discarded", async () => {
    const { result } = await loaded();
    act(() => result.current.setValue({ settings: { workers: 4 } }));
    expect(result.current.text).toContain("workers: 4 # tasks");
    expect(result.current.changes).toBe(1);
    act(() => result.current.discard());
    expect(result.current.text).toBe(FILE);
    expect(result.current.changes).toBe(0);
  });

  it("a Source edit that does not parse reports the error and disables nothing silently", async () => {
    const { result } = await loaded();
    act(() => result.current.setText("settings: [1,"));
    expect(result.current.parseError).toBeTruthy();
    expect(result.current.changes).toBe(0);
  });

  it("a form edit while the YAML does not parse leaves the text alone instead of throwing", async () => {
    const { result } = await loaded();
    act(() => result.current.setText("settings: [1,"));
    act(() => result.current.setValue({ settings: { workers: 4 } }));
    expect(result.current.text).toBe("settings: [1,");
  });

  it("no config yet (404) starts from an empty document", async () => {
    api.getCurrentConfig.mockRejectedValue(new ApiError(404, "none"));
    const { result } = await loaded();
    expect(result.current.revision).toBeNull();
    expect(result.current.value).toEqual({});
  });

  it("apply sends the loaded revision as the base and a 409 says the edit is stale", async () => {
    const { result } = await loaded();
    act(() => result.current.setValue({ settings: { workers: 4 } }));
    api.submitConfig.mockRejectedValueOnce(new ApiError(409, "stale"));
    expect(await result.current.apply()).toMatchObject({ ok: false, stale: true });
    expect(api.submitConfig).toHaveBeenCalledWith(expect.stringContaining("workers: 4"), { base: 7 });
    api.submitConfig.mockResolvedValueOnce({ revision: 8 });
    let r: any;
    await act(async () => (r = await result.current.apply()));
    expect(r).toEqual({ ok: true, revision: 8 });
    expect(result.current.revision).toBe(8);
    expect(result.current.changes).toBe(0);
  });

  it("apply passes canary and confirm options along with the base", async () => {
    const { result } = await loaded();
    api.submitConfig.mockResolvedValueOnce({ revision: 8 });
    await act(async () => void (await result.current.apply({ canaryGroup: "eu", confirmWithin: 60 })));
    expect(api.submitConfig).toHaveBeenCalledWith(FILE, { canaryGroup: "eu", confirmWithin: 60, base: 7 });
  });

  it("raises schema_version when a form edit sets a field newer than the document", async () => {
    api.getConfigSchema.mockResolvedValue({
      type: "object",
      properties: { settings: { type: "object", properties: { workers: { type: "integer" }, fancy: { type: "boolean", "x-wayhouse-since": 4 } } } },
    });
    const { result } = await loaded();
    act(() => result.current.setValue({ settings: { workers: 2, fancy: true } }));
    expect(result.current.text).toContain("schema_version: 4");
    act(() => result.current.setValue({ settings: { workers: 3, fancy: true }, schema_version: 4 }));
    expect(result.current.text.match(/schema_version/g)).toHaveLength(1);
  });

  it("validate returns the controller's message", async () => {
    const { result } = await loaded();
    api.validateConfig.mockRejectedValueOnce(new ApiError(422, "pool eu has no targets"));
    expect(await result.current.validate()).toEqual({ ok: false, message: "pool eu has no targets" });
  });
});
