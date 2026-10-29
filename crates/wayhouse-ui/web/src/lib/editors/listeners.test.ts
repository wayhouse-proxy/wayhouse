import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { DEFAULT_LAYOUT } from "../form/layout";
import { MATCH_TYPES, changeMatchType, describeRoute, moveRoute, newListener, newRoute, routeTarget, uniqueName } from "./listeners";

describe("listener helpers", () => {
  it("picks a name not used yet", () => {
    expect(uniqueName("listener", [])).toBe("listener-1");
    expect(uniqueName("listener", ["listener-1", "other"])).toBe("listener-2");
  });

  it("starts a listener with only what is required", () => {
    expect(newListener([{ name: "listener-1" }])).toEqual({ name: "listener-2", bind: "0.0.0.0:25565", protocol: "tcp" });
  });

  it("starts a route that matches everything and sends it to the first pool", () => {
    expect(newRoute(["lobby", "eu"])).toEqual({ match: { type: "always" }, action: { pool: "lobby" } });
    expect(newRoute([])).toEqual({ match: { type: "always" }, action: {} });
  });

  it("changing the match type keeps only the fields the new type takes", () => {
    const m = { type: "sni", host: ["a.example"] };
    expect(changeMatchType(m, "port")).toEqual({ type: "port" });
    expect(changeMatchType({ type: "sniffer", sniffer: "minecraft", host: ["x"] }, "sni")).toEqual({ type: "sni", host: ["x"] });
    expect(changeMatchType({ type: "first_bytes", prefix: "hex:ff", length: { min: 1, max: 2 } }, "always")).toEqual({ type: "always" });
  });

  it("every match type names the fields it takes, and they are the fields the config parser accepts", () => {
    expect(Object.keys(MATCH_TYPES).sort()).toEqual(["always", "client_cidr", "dst", "first_bytes", "port", "sni", "sniffer"]);
    expect(MATCH_TYPES.sniffer.fields).toEqual(["sniffer", "host"]);
    expect(MATCH_TYPES.always.fields).toEqual([]);
  });

  it("moves a route up or down and not past the ends", () => {
    const r = [{ n: 1 }, { n: 2 }, { n: 3 }];
    expect(moveRoute(r, 2, -1).map((x) => x.n)).toEqual([1, 3, 2]);
    expect(moveRoute(r, 0, -1)).toBe(r);
    expect(moveRoute(r, 2, 1)).toBe(r);
  });

  it("names a route's target and says in words what it does", () => {
    expect(routeTarget({ action: { pool: "lobby" } })).toEqual({ kind: "pool", name: "lobby" });
    expect(routeTarget({ action: { resolver: "r1" } })).toEqual({ kind: "resolver", name: "r1" });
    expect(routeTarget({ action: {} })).toBeNull();
    expect(describeRoute({ match: { type: "sni", host: ["a.example", "*.b.example"] }, action: { pool: "eu" } })).toBe("SNI a.example, *.b.example to pool eu");
    expect(describeRoute({ match: { type: "port", ports: [30001, "30010-30020"] }, action: { resolver: "r" } })).toBe("destination port 30001, 30010-30020 to resolver r");
    expect(describeRoute({ match: { type: "always" }, action: { pool: "p" } })).toBe("everything to pool p");
    expect(describeRoute({ match: { type: "always" }, action: {} })).toBe("everything to nothing yet");
  });
});

describe("coverage guard: every listener, route and match field is rendered, hand-edited or allowlisted", () => {
  const schema = JSON.parse(readFileSync(resolve(process.cwd(), "../../wayhouse-config/schema/config.schema.json"), "utf8"));
  const props = (def: string) => Object.keys(schema.$defs[def].properties);
  /** Fields of a struct that the generated form shows on the listener page (default layout, `page: "listener"`). */
  const form = (prefix: string) =>
    new Set(
      Object.entries(DEFAULT_LAYOUT.fields)
        .filter(([k, f]) => f.page === "listener" && k.startsWith(prefix))
        .map(([k]) => k.slice(prefix.length).split(".")[0]),
    );
  /** Fields only a hand editor covers: the routes list. Nothing is allowlisted today; add here with the reason if a field must stay YAML-only. */
  const HAND = { listener: ["routes"], route: ["match", "action"], match: [...Object.values(MATCH_TYPES).flatMap((t) => t.fields as string[]), "type"], action: ["pool", "resolver"] } as const;
  const ALLOWLIST: Record<string, string[]> = { listener: [], route: [], match: [], action: [] };

  const missing = (def: string, key: keyof typeof HAND, formFields: Set<string>) =>
    props(def).filter((p) => !formFields.has(p) && !(HAND[key] as readonly string[]).includes(p) && !ALLOWLIST[key].includes(p));

  it("RawListener", () => expect(missing("RawListener", "listener", form("listeners.*."))).toEqual([]));
  it("RawRoute", () => expect(missing("RawRoute", "route", new Set())).toEqual([]));
  it("RawMatch", () => expect(missing("RawMatch", "match", new Set())).toEqual([]));
  it("RawAction", () => expect(missing("RawAction", "action", new Set())).toEqual([]));
  it("nested listener objects: every field of geo, per_source, rate_limit and plugin_routes is on the listener page", () => {
    const f = (name: string, def: string) => props(def).filter((p) => !(DEFAULT_LAYOUT.fields[`listeners.*.${name}.${p}`]?.page === "listener" || Object.keys(DEFAULT_LAYOUT.fields).some((k) => k.startsWith(`listeners.*.${name}.${p}.`) && DEFAULT_LAYOUT.fields[k].page === "listener")));
    expect([f("geo", "RawGeo"), f("per_source", "RawPerSource"), f("rate_limit", "RawRateLimit"), f("plugin_routes", "RawPluginRoutes")]).toEqual([[], [], [], []]);
  });
});
