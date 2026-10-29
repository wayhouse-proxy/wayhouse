import { useEffect, useState } from "react";
import { ArrowDown, ArrowUp, Plus, Trash2 } from "lucide-react";
import { MATCH_TYPES, changeMatchType, moveRoute, newRoute, type Json } from "../../lib/editors/listeners";
import { Button, Input } from "../ui/Button";
import { NumberInput, Select, Textarea } from "../ui/Input";

const fromLines = (s: string): string[] | undefined => {
  const out = s.split("\n").map((l) => l.trim()).filter(Boolean);
  return out.length ? out : undefined;
};
/** A bare integer stays an integer (`30001`), a range stays text (`"30010-30020"`). */
const toPort = (s: string): number | string => (/^\d+$/.test(s) ? Number(s) : s);

/** One value per line. The text is kept as typed (a new empty line must survive) and re-read only when the list changes elsewhere. */
function LinesArea({ list, onList, ...rest }: { list: unknown[]; onList: (lines: string[] | undefined) => void } & Omit<React.ComponentProps<typeof Textarea>, "value" | "onChange">) {
  const [text, setText] = useState(list.join("\n"));
  useEffect(() => {
    setText((t) => (JSON.stringify(fromLines(t) ?? []) === JSON.stringify(list.map(String)) ? t : list.join("\n")));
  }, [list]);
  return (
    <Textarea
      {...rest}
      value={text}
      onChange={(e) => {
        setText(e.target.value);
        onList(fromLines(e.target.value));
      }}
    />
  );
}

const LABEL: Record<string, string> = { cidrs: "Prefixes (CIDR), one per line", ports: "Ports, one per line", host: "Host patterns, one per line", prefix: "Prefix (hex:… or ascii:…)", length: "Length", sniffer: "Sniffer name" };

/**
 * The routes of one listener, in the order they are tried (the first match wins). A hand editor because a route's match is
 * one flat shape whose valid fields depend on its type: the type picker shows only the fields that type takes.
 */
export function RouteEditor({ routes, pools, resolvers, readOnly, onChange }: { routes: Json[]; pools: string[]; resolvers: string[]; readOnly: boolean; onChange: (routes: Json[]) => void }) {
  const set = (i: number, route: Json) => onChange(routes.map((r, j) => (j === i ? route : r)));

  return (
    <section aria-label="Routes" className="mb-4">
      <h3 className="mb-1 text-lg font-semibold">Routes</h3>
      <p className="mb-2 max-w-(--measure) text-sm text-ink-muted">Tried from the top; the first route that matches decides. Put a route that matches everything last.</p>
      {routes.length === 0 && <p className="mb-2 text-sm text-ink-muted">No routes. The listener forwards through its pool shorthand, or nothing.</p>}
      {routes.map((route, i) => {
        const match: Json = route.match ?? { type: "always" };
        const action: Json = route.action ?? {};
        const kind = action.resolver !== undefined && action.resolver !== null ? "resolver" : "pool";
        const names = kind === "pool" ? pools : resolvers;
        const target: string = action[kind] ?? "";
        const spec = MATCH_TYPES[match.type];
        return (
          <div key={i} role="group" aria-label={`Route ${i + 1}`} className="mb-2 border border-line bg-surface p-3">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-semibold tabular-nums">{i + 1}.</span>
              <label className="flex items-center gap-1.5 text-sm text-ink-muted">
                Match
                <Select aria-label={`Match type of route ${i + 1}`} value={match.type} disabled={readOnly} onChange={(e) => set(i, { ...route, match: changeMatchType(match, e.target.value) })}>
                  {!spec && <option value={match.type}>{match.type} (unknown)</option>}
                  {Object.entries(MATCH_TYPES).map(([k, t]) => (
                    <option key={k} value={k}>
                      {t.label}
                    </option>
                  ))}
                </Select>
              </label>
              <label className="flex items-center gap-1.5 text-sm text-ink-muted">
                Send to
                <Select
                  aria-label={`Target kind of route ${i + 1}`}
                  value={kind}
                  disabled={readOnly}
                  onChange={(e) => set(i, { ...route, action: e.target.value === "pool" ? {} : { resolver: "" } })}
                >
                  <option value="pool">pool</option>
                  <option value="resolver">resolver</option>
                </Select>
                <Select
                  aria-label={`Target of route ${i + 1}`}
                  value={target}
                  disabled={readOnly}
                  onChange={(e) => set(i, { ...route, action: e.target.value ? { [kind]: e.target.value } : {} })}
                >
                  <option value="">choose a {kind}</option>
                  {target && !names.includes(target) && <option value={target}>{target} (not defined)</option>}
                  {names.map((n) => (
                    <option key={n}>{n}</option>
                  ))}
                </Select>
              </label>
              <span className="ml-auto flex gap-1">
                <Button size="sm" aria-label={`Move route ${i + 1} up`} disabled={readOnly || i === 0} onClick={() => onChange(moveRoute(routes, i, -1))}>
                  <ArrowUp aria-hidden size={14} strokeWidth={1.5} />
                </Button>
                <Button size="sm" aria-label={`Move route ${i + 1} down`} disabled={readOnly || i === routes.length - 1} onClick={() => onChange(moveRoute(routes, i, 1))}>
                  <ArrowDown aria-hidden size={14} strokeWidth={1.5} />
                </Button>
                <Button size="sm" variant="danger" aria-label={`Remove route ${i + 1}`} disabled={readOnly} onClick={() => onChange(routes.filter((_, j) => j !== i))}>
                  <Trash2 aria-hidden size={14} strokeWidth={1.5} />
                </Button>
              </span>
            </div>
            {spec && <p className="mt-1 text-xs text-ink-muted">{spec.help}</p>}
            <div className="mt-2 flex flex-wrap gap-3">
              {spec?.fields.map((f) => {
                const label = `${LABEL[f]} (route ${i + 1})`;
                const put = (v: unknown) => {
                  const next = { ...match };
                  if (v === undefined) delete next[f];
                  else next[f] = v;
                  set(i, { ...route, match: next });
                };
                if (f === "cidrs" || f === "host" || f === "ports")
                  return (
                    <label key={f} className="flex flex-col gap-0.5 text-xs text-ink-muted">
                      {LABEL[f]}
                      <LinesArea
                        aria-label={label}
                        rows={3}
                        spellCheck={false}
                        disabled={readOnly}
                        className="w-64 font-mono"
                        list={Array.isArray(match[f]) ? match[f] : []}
                        onList={(v) => put(v && f === "ports" ? v.map(toPort) : v)}
                      />
                    </label>
                  );
                if (f === "length")
                  return (
                    <fieldset key={f} className="flex items-end gap-2 text-xs text-ink-muted">
                      <legend className="mb-0.5">Length in bytes</legend>
                      {(["min", "max"] as const).map((k) => (
                        <label key={k} className="flex flex-col gap-0.5">
                          {k}
                          <NumberInput
                            aria-label={`Length ${k} (route ${i + 1})`}
                            disabled={readOnly}
                            min={0}
                            value={match.length?.[k] ?? ""}
                            onChange={(e) => {
                              const n = e.target.value === "" ? undefined : Number(e.target.value);
                              const len = { ...match.length, [k]: n };
                              put(len.min === undefined && len.max === undefined ? undefined : len);
                            }}
                          />
                        </label>
                      ))}
                    </fieldset>
                  );
                return (
                  <label key={f} className="flex flex-col gap-0.5 text-xs text-ink-muted">
                    {LABEL[f]}
                    <Input aria-label={label} mono disabled={readOnly} className="w-64" value={match[f] ?? ""} onChange={(e) => put(e.target.value === "" ? undefined : e.target.value)} />
                  </label>
                );
              })}
            </div>
          </div>
        );
      })}
      <Button disabled={readOnly} onClick={() => onChange([...routes, newRoute(pools)])}>
        <Plus aria-hidden size={14} strokeWidth={1.5} /> Add route
      </Button>
    </section>
  );
}
