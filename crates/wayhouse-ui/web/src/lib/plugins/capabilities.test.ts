import { describe, expect, it } from "vitest";
import type { PluginCapabilities } from "../../types";
import { describeCapabilities, unknownCapabilityKeys } from "./capabilities";

const base: PluginCapabilities = { triggers: { on_timer: false }, tick_interval_secs: 0, log: false, state: null };

describe("describeCapabilities", () => {
  it("says a module with nothing declared asks for nothing", () => {
    expect(describeCapabilities(base)).toEqual(["Asks for nothing"]);
  });

  it("puts hosts, ports, private access, secrets, routes and the other triggers in words", () => {
    const lines = describeCapabilities({
      ...base,
      triggers: { on_timer: true, on_webhook: true, on_event: ["backend.down"], on_acme_dns: true },
      tick_interval_secs: 30,
      http: { hosts: [{ host: "panel.example" }, { host: "10.0.0.5", port: 8443, allow_private: true }] },
      secrets: [{ name: "PANEL_TOKEN", description: "panel API key", hosts: ["panel.example"] }],
      routes: { hosts: ["*.play.example"], backends: ["10.0.0.0/8"], max_entries: 50 },
    });
    expect(lines).toEqual([
      "Runs every 30 s",
      "Can be called through a webhook you enable for it",
      "Is told about wayhouse events: backend.down",
      "Sets and removes the DNS records certificate validation needs",
      "Makes HTTPS requests to panel.example",
      "Makes HTTPS requests to 10.0.0.5:8443, including private and local addresses",
      "Sends the secret PANEL_TOKEN (panel API key) only to panel.example",
      "Claims up to 50 hostnames (*.play.example) pointing into 10.0.0.0/8",
    ]);
  });

  it("still shows what it has no wording for", () => {
    const caps = { ...base, gpu: true, triggers: { on_timer: false, on_gpu: true } } as unknown as PluginCapabilities;
    expect(unknownCapabilityKeys(caps)).toEqual(["gpu", "triggers.on_gpu"]);
    expect(describeCapabilities(caps)).toEqual(["Also asks for: gpu", "Also asks for: triggers.on_gpu"]);
  });
});
