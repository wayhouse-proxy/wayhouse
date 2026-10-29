import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { DEFAULT_LAYOUT } from "../form/layout";
import { SOURCE_TYPES, newPool, newResolver, newSource, poolFields, setPoolSource, sourceFields, tidyPool, usedBy } from "./lists";

describe("pool, source and resolver helpers", () => {
  it("starts each with only what names it", () => {
    expect(newPool([{ name: "pool-1" }])).toEqual({ name: "pool-2", targets: [] });
    expect(newSource([])).toEqual({ name: "source-1", type: "static", targets: [] });
    expect(newResolver([])).toEqual({ name: "resolver-1", type: "http", endpoint: "" });
  });

  it("a source type shows the fields it takes and no others", () => {
    expect(sourceFields("static")).toEqual(["name", "refresh_interval_sec", "targets"]);
    expect(sourceFields("dns_srv")).toContain("record");
    expect(sourceFields("dns_srv")).not.toContain("targets");
    expect(sourceFields("consul")).toEqual(expect.arrayContaining(["service", "tag", "consul_addr", "consul_token_file"]));
    expect(sourceFields("kubernetes")).toEqual(expect.arrayContaining(["service", "namespace", "port_name", "api"]));
    expect(sourceFields("tunnel")).toContain("pubkey");
    expect(SOURCE_TYPES.map((t) => t.value)).toEqual(["static", "dns_srv", "consul", "kubernetes", "tunnel"]);
  });

  it("a pool shows balancer options only where they apply, and the static list only without a source", () => {
    expect(poolFields({ balancer: "round_robin" })).not.toContain("weights");
    expect(poolFields({ balancer: "weighted" })).toContain("weights");
    expect(poolFields({ balancer: "consistent_hash" })).toContain("hash_on");
    expect(poolFields({})).toContain("targets");
    expect(poolFields({ source: "dns" })).not.toContain("targets");
  });

  it("leaving the weighted or consistent-hash balancer drops the fields only they take (the config parser refuses them)", () => {
    const stale = { name: "p", balancer: "round_robin", weights: { "a:1": 2 }, hash_on: "src_ip", targets: ["a:1"] };
    expect(tidyPool(stale)).toEqual({ name: "p", balancer: "round_robin", targets: ["a:1"] });
    expect(tidyPool({ ...stale, balancer: "weighted" })).toEqual({ name: "p", balancer: "weighted", weights: { "a:1": 2 }, targets: ["a:1"] });
    expect(tidyPool({ ...stale, balancer: "consistent_hash" })).toEqual({ name: "p", balancer: "consistent_hash", hash_on: "src_ip", targets: ["a:1"] });
    expect(tidyPool({ name: "p", targets: [] })).toEqual({ name: "p", targets: [] });
  });

  it("choosing a source drops the static list; choosing the list again drops the source", () => {
    expect(setPoolSource({ name: "p", targets: ["a:1"] }, "dns")).toEqual({ name: "p", source: "dns" });
    expect(setPoolSource({ name: "p", source: "dns" }, "")).toEqual({ name: "p", targets: [] });
  });

  it("says where a pool, resolver or source is used", () => {
    const cfg = {
      listeners: [
        { name: "a", pool: "lobby", routes: [{ match: { type: "always" }, action: { pool: "eu" } }, { match: { type: "always" }, action: { resolver: "r" } }] },
        { name: "b", routes: [{ match: { type: "always" }, action: { pool: "lobby" } }] },
      ],
      pools: [{ name: "lobby", source: "dns" }, { name: "eu" }],
    };
    expect(usedBy(cfg, "pool", "lobby")).toEqual(["listener a", "listener b route 1"]);
    expect(usedBy(cfg, "pool", "eu")).toEqual(["listener a route 1"]);
    expect(usedBy(cfg, "resolver", "r")).toEqual(["listener a route 2"]);
    expect(usedBy(cfg, "source", "dns")).toEqual(["pool lobby"]);
    expect(usedBy(cfg, "pool", "nobody")).toEqual([]);
  });
});

describe("coverage guard: every pool, source and resolver field is rendered, hand-edited or allowlisted", () => {
  const schema = JSON.parse(readFileSync(resolve(process.cwd(), "../../wayhouse-config/schema/config.schema.json"), "utf8"));
  const props = (def: string) => Object.keys(schema.$defs[def].properties);
  const onPage = (prefix: string, page: string) =>
    new Set(
      Object.entries(DEFAULT_LAYOUT.fields)
        .filter(([k, f]) => f.page === page && k.startsWith(prefix))
        .map(([k]) => k.slice(prefix.length).split(".")[0]),
    );
  /** Hand-edited: the pool's backend source (a select over the defined sources), the source and resolver `type`. */
  const HAND: Record<string, string[]> = { pool: ["source"], source: ["type"], resolver: ["type"] };
  const missing = (def: string, key: string, prefix: string) => props(def).filter((p) => !onPage(prefix, key).has(p) && !HAND[key].includes(p));
  it("RawPool", () => expect(missing("RawPool", "pool", "pools.*.")).toEqual([]));
  it("RawBackendSource", () => expect(missing("RawBackendSource", "source", "backend_sources.*.")).toEqual([]));
  it("RawResolver", () => expect(missing("RawResolver", "resolver", "resolvers.*.")).toEqual([]));
  it("nested: health_check, per_backend and cache fields", () => {
    const nested = (def: string, parent: string, page: string, prefix: string) => props(def).filter((p) => DEFAULT_LAYOUT.fields[`${prefix}${parent}.${p}`]?.page !== page);
    expect([nested("RawHealthCheck", "health_check", "pool", "pools.*."), nested("RawPerBackend", "per_backend", "pool", "pools.*."), nested("RawCache", "cache", "resolver", "resolvers.*.")]).toEqual([[], [], []]);
  });
  it("the hand-written field lists leave no schema field out: a new one fails here until it is placed", () => {
    const poolShown = new Set(["weighted", "consistent_hash", "round_robin"].flatMap((balancer) => [...poolFields({ balancer }), ...poolFields({ balancer, source: "s" })]));
    expect(props("RawPool").filter((p) => !poolShown.has(p) && !HAND.pool.includes(p))).toEqual([]);
    const sourceShown = new Set(SOURCE_TYPES.flatMap((t) => sourceFields(t.value)));
    expect(props("RawBackendSource").filter((p) => !sourceShown.has(p) && !HAND.source.includes(p))).toEqual([]);
  });

  it("every source type's fields are real RawBackendSource fields", () => {
    const real = new Set(props("RawBackendSource"));
    for (const t of SOURCE_TYPES) expect(sourceFields(t.value).filter((f) => !real.has(f))).toEqual([]);
  });
});
