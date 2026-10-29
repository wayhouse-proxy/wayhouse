import { useEffect, useRef, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearchParams } from "react-router-dom";
import { ArrowLeft, ArrowRight, Settings2, X } from "lucide-react";
import { WidgetBoundary } from "../components/dashboard/WidgetFrame";
import { FleetFeed, WIDGETS, widgetById, rangeOf, topOf, type WidgetDef } from "../components/dashboard/widgets";
import { useInspector } from "../components/shell/Inspector";
import { PageHeader } from "../components/shell/Page";
import { ErrorState, Loading } from "../components/shell/States";
import { Button } from "../components/ui/Button";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { FieldRow } from "../components/ui/FieldRow";
import { Input, Select } from "../components/ui/Input";
import { useToast } from "../components/ui/Toast";
import {
  DEFAULT_DASHBOARD,
  MAX_WIDGETS,
  WIDTHS,
  dashboardNameProblem,
  deleteDashboard,
  exportDashboard,
  listDashboards,
  move,
  parseDashboard,
  saveDashboard,
  type Dashboard,
  type PlacedWidget,
} from "../lib/dashboard/model";
import { useCan } from "../lib/session";
import { RANGES } from "../lib/stats";

/** Tailwind needs whole class names, so the 12 spans are spelled out. Under 760 px every widget is full width. */
const SPAN: Record<number, string> = {
  1: "min-[760px]:col-span-1", 2: "min-[760px]:col-span-2", 3: "min-[760px]:col-span-3", 4: "min-[760px]:col-span-4",
  5: "min-[760px]:col-span-5", 6: "min-[760px]:col-span-6", 7: "min-[760px]:col-span-7", 8: "min-[760px]:col-span-8",
  9: "min-[760px]:col-span-9", 10: "min-[760px]:col-span-10", 11: "min-[760px]:col-span-11", 12: "min-[760px]:col-span-12",
};
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

function Unavailable({ id }: { id: string }) {
  return (
    <section aria-label="Unavailable widget" className="flex h-full flex-col rounded-sm border border-dashed border-line bg-surface-sunken p-3 text-sm">
      <p className="font-semibold">This widget is not available</p>
      <p className="mt-1 text-ink-muted">
        This build does not know <span className="font-mono">{id}</span>. Remove it, or keep it for a build that does.
      </p>
    </section>
  );
}

function SettingsForm({ def, w, onChange }: { def: WidgetDef; w: PlacedWidget; onChange: (patch: Record<string, unknown>) => void }) {
  const s = w.settings ?? {};
  return (
    <div className="flex flex-col gap-3">
      {def.settings.map((d) =>
        d.key === "range" ? (
          <FieldRow
            key={d.key}
            label={d.label}
            render={(a) => (
              <Select {...a} value={rangeOf(s)} onChange={(e) => onChange({ range: e.target.value })}>
                {RANGES.map((r) => (
                  <option key={r.value} value={r.value}>
                    {r.label}
                  </option>
                ))}
              </Select>
            )}
          />
        ) : (
          <FieldRow
            key={d.key}
            label={d.label}
            render={(a) => (
              <Select {...a} value={topOf(s)} onChange={(e) => onChange({ top: Number(e.target.value) })}>
                {[3, 5, 10, 20].map((n) => (
                  <option key={n}>{n}</option>
                ))}
              </Select>
            )}
          />
        ),
      )}
    </div>
  );
}

/** The first page after sign-in: named dashboards on a 12-column grid, with a button-only customize mode (components.md). */
export function DashboardPage() {
  const stored = useQuery({ queryKey: ["ui", "dashboards"], queryFn: listDashboards });
  if (stored.isPending) return <Loading rows={4} />;
  if (stored.error) return <ErrorState onRetry={() => stored.refetch()}>{message(stored.error)}</ErrorState>;
  return <Board stored={stored.data} />;
}

function Board({ stored: storedList }: { stored: Dashboard[] }) {
  const canEdit = useCan("operator");
  const toast = useToast();
  const qc = useQueryClient();
  const { confirm, dialog } = useConfirm();
  const inspector = useInspector();
  const [params, setParams] = useSearchParams();
  const [draft, setDraft] = useState<Dashboard | null>(null);
  const [naming, setNaming] = useState<{ mode: "new" | "rename"; value: string } | null>(null);
  const [showJson, setShowJson] = useState(false);
  const [settingsFor, setSettingsFor] = useState<string | null>(null);
  const [said, setSaid] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const file = useRef<HTMLInputElement>(null);

  const save = useMutation({ mutationFn: saveDashboard, onSuccess: () => qc.invalidateQueries({ queryKey: ["ui", "dashboards"] }) });
  const remove = useMutation({ mutationFn: deleteDashboard, onSuccess: () => qc.invalidateQueries({ queryKey: ["ui", "dashboards"] }) });

  const list = storedList.length ? storedList : [DEFAULT_DASHBOARD];
  const unsaved = storedList.length === 0;
  const wanted = params.get("d");
  const savedBoard = list.find((d) => d.name === wanted) ?? list[0];
  const board = draft ?? savedBoard;
  const customizing = draft !== null;
  // Keyed by widget id so a move keeps each widget (and keyboard focus) mounted; a repeated id gets its occurrence number.
  const seen = new Map<string, number>();
  const keys = board.widgets.map((w) => {
    const n = (seen.get(w.id) ?? 0) + 1;
    seen.set(w.id, n);
    return n === 1 ? w.id : `${w.id}#${n}`;
  });
  const names = list.map((d) => d.name);

  const select = (name: string) => setParams((p) => (p.set("d", name), p), { replace: true });
  const run = async (fn: () => Promise<unknown>) => {
    setProblem(null);
    try {
      await fn();
      return true;
    } catch (e) {
      setProblem(message(e));
      return false;
    }
  };
  const edit = (f: (w: PlacedWidget[]) => PlacedWidget[]) => setDraft((d) => ({ ...(d ?? board), widgets: f((d ?? board).widgets) }));
  const stopCustomizing = () => {
    setDraft(null);
    setSaid("");
    setShowJson(false);
    setSettingsFor(null);
    inspector.close();
  };

  async function done() {
    if (await run(() => save.mutateAsync(board))) {
      toast("Dashboard saved");
      stopCustomizing();
    }
  }
  async function create(name: string) {
    const next: Dashboard = naming?.mode === "rename" ? { ...savedBoard, name } : { name, widgets: [] };
    if (!(await run(() => save.mutateAsync(next)))) return;
    if (naming?.mode === "rename" && !unsaved && !(await run(() => remove.mutateAsync(savedBoard.name)))) {
      setProblem(`The renamed copy "${name}" was saved but the old dashboard "${savedBoard.name}" still exists: delete it by hand.`);
      return;
    }
    setNaming(null);
    select(name);
    if (next.widgets.length === 0) setDraft(next);
    toast(naming?.mode === "rename" ? "Dashboard renamed" : "Dashboard created");
  }
  async function del() {
    const ok = await confirm({ title: `Delete ${savedBoard.name}?`, description: "It is removed for every operator of this deployment.", confirmLabel: "Delete dashboard" });
    if (ok && (await run(() => remove.mutateAsync(savedBoard.name)))) {
      params.delete("d");
      setParams(params, { replace: true });
      stopCustomizing();
      toast("Dashboard deleted");
    }
  }
  function download() {
    const url = URL.createObjectURL(new Blob([exportDashboard(board)], { type: "application/json" }));
    const a = Object.assign(document.createElement("a"), { href: url, download: `${board.name}.json` });
    a.click();
    URL.revokeObjectURL(url);
  }
  async function importFile(f: File | undefined) {
    if (!f) return;
    if (file.current) file.current.value = "";
    setProblem(null);
    try {
      const d = parseDashboard(await f.text());
      let name = d.name || f.name.replace(/\.json$/i, "");
      for (let n = 2; dashboardNameProblem(name, names); n++) {
        if (!names.includes(name) && dashboardNameProblem(name)) throw new Error(dashboardNameProblem(name)!);
        name = `${d.name || "Imported"} ${n}`;
      }
      if (await run(() => save.mutateAsync({ ...d, name }))) {
        select(name);
        toast(`Imported as ${name}`);
      }
    } catch (e) {
      setProblem(message(e));
    }
  }
  const titleOf = (w: PlacedWidget) => widgetById(w.id)?.title ?? w.id;
  const shift = (i: number, by: number) => {
    const to = Math.max(0, Math.min(board.widgets.length - 1, i + by));
    edit((all) => move(all, i, by));
    setSaid(`${titleOf(board.widgets[i])} moved to position ${to + 1} of ${board.widgets.length}`);
  };
  const drop = (i: number) => {
    setSaid(`${titleOf(board.widgets[i])} removed`);
    edit((all) => all.filter((_, j) => j !== i));
    // The removed widget took the focused button with it; the Done button is the nearest stable place.
    requestAnimationFrame(() => document.querySelector<HTMLElement>("[data-done]")?.focus());
  };
  const showSettings = (key: string) => {
    const i = keys.indexOf(key);
    const w = board.widgets[i];
    const def = w && widgetById(w.id);
    if (!def) {
      setSettingsFor(null);
      inspector.close();
      return;
    }
    inspector.open(def.title, <SettingsForm def={def} w={w} onChange={(patch) => edit((all) => all.map((x, j) => (j === i ? { ...x, settings: { ...x.settings, ...patch } } : x)))} />);
  };
  // The inspector holds a snapshot, so a change re-opens it with the current values while it is showing.
  useEffect(() => {
    if (settingsFor !== null && inspector.current) showSettings(settingsFor);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft]);

  const free = WIDGETS.filter((d) => !board.widgets.some((w) => w.id === d.id));
  const nameError = naming ? dashboardNameProblem(naming.value, naming.mode === "rename" ? names.filter((n) => n !== savedBoard.name) : names) : null;
  const writeTip = canEdit ? undefined : "Changing a dashboard needs the operator role";

  return (
    <div className="flex min-h-0 flex-1">
      <div className="min-w-0 flex-1">
        {dialog}
        <PageHeader
          title="Dashboard"
          summary={unsaved ? "Showing the default dashboard. Customize it and press Done to store it for everyone." : undefined}
          actions={
            customizing ? (
              <>
                <Button variant="primary" data-done onClick={done} loading={save.isPending}>
                  Done
                </Button>
                <Button onClick={() => setDraft(savedBoard)}>Reset</Button>
                <Button onClick={() => setShowJson(!showJson)} aria-pressed={showJson}>
                  Show JSON
                </Button>
                <Button variant="ghost" onClick={stopCustomizing}>
                  Cancel
                </Button>
              </>
            ) : (
              <Button disabled={!canEdit} title={writeTip} onClick={() => setDraft(savedBoard)}>
                Customize
              </Button>
            )
          }
        />
        <div className="mb-3 flex flex-wrap items-end gap-2">
          <FieldRow
            label="Dashboard"
            render={(a) => (
              <Select {...a} value={savedBoard.name} disabled={customizing} onChange={(e) => select(e.target.value)}>
                {list.map((d) => (
                  <option key={d.name}>{d.name}</option>
                ))}
              </Select>
            )}
          />
          <Button size="sm" disabled={!canEdit || customizing} title={writeTip} onClick={() => setNaming({ mode: "new", value: "" })}>
            New
          </Button>
          <Button size="sm" disabled={!canEdit || customizing || unsaved} title={writeTip} onClick={() => setNaming({ mode: "rename", value: savedBoard.name })}>
            Rename
          </Button>
          <Button size="sm" variant="danger" disabled={!canEdit || customizing || unsaved} title={writeTip} onClick={del}>
            Delete
          </Button>
          <Button size="sm" onClick={download}>
            Export
          </Button>
          <Button size="sm" disabled={!canEdit} title={writeTip} onClick={() => file.current?.click()}>
            Import
          </Button>
          <input ref={file} type="file" accept="application/json,.json" aria-label="Import a dashboard file" className="hidden" onChange={(e) => importFile(e.target.files?.[0])} />
        </div>
        {naming && (
          <form
            className="mb-3 flex flex-wrap items-start gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (!nameError) create(naming.value);
            }}
          >
            <FieldRow label={naming.mode === "new" ? "New dashboard name" : "New name"} error={naming.value && nameError ? nameError : undefined} render={(a) => <Input {...a} value={naming.value} onChange={(e) => setNaming({ ...naming, value: e.target.value })} />} />
            <Button type="submit" variant="primary" size="sm" disabled={!!nameError}>
              {naming.mode === "new" ? "Create" : "Rename"}
            </Button>
            <Button size="sm" variant="ghost" onClick={() => setNaming(null)}>
              Cancel
            </Button>
          </form>
        )}
        {problem && <ErrorState>{problem}</ErrorState>}
        {customizing && (
          <div className="mb-3 flex flex-wrap items-end gap-2 border border-dashed border-line px-3 py-2">
            <FieldRow
              label="Add widget"
              render={(a) => (
                <Select
                  {...a}
                  value=""
                  disabled={free.length === 0 || board.widgets.length >= MAX_WIDGETS}
                  onChange={(e) => {
                    const d = widgetById(e.target.value);
                    if (d) {
                      edit((all) => [...all, { id: d.id, width: d.defaultWidth }]);
                      setSaid(`${d.title} added`);
                    }
                  }}
                >
                  <option value="">{free.length ? "Choose a widget" : "All widgets are on the dashboard"}</option>
                  {free.map((d) => (
                    <option key={d.id} value={d.id}>
                      {d.title}
                    </option>
                  ))}
                </Select>
              )}
            />
          </div>
        )}
        {customizing && showJson && (
          <pre aria-label="Dashboard JSON" className="mb-3 overflow-auto border border-line bg-surface-sunken p-3 font-mono text-xs">
            {exportDashboard(board)}
          </pre>
        )}
        {board.widgets.length === 0 ? (
          <p className="py-8 text-sm text-ink-muted">{customizing ? "No widgets yet. Add one above." : "This dashboard has no widgets. Press Customize to add some."}</p>
        ) : (
          <FleetFeed>
          <div className="grid grid-cols-12 gap-3">
            {board.widgets.map((w, i) => {
              const def = widgetById(w.id);
              const View = def?.Component;
              return (
                <div key={keys[i]} className={`col-span-12 ${SPAN[w.width] ?? SPAN[6]} ${customizing ? "outline-1 outline-dashed outline-offset-2 outline-line" : ""}`}>
                  {customizing && (
                    <div role="group" aria-label={`${def?.title ?? w.id} tools`} className="mb-1 flex flex-wrap items-center gap-1">
                      <Button size="sm" variant="ghost" aria-label={`Move ${def?.title ?? w.id} earlier`} disabled={i === 0} onClick={() => shift(i, -1)}>
                        <ArrowLeft aria-hidden size={14} strokeWidth={1.5} />
                      </Button>
                      <Button size="sm" variant="ghost" aria-label={`Move ${def?.title ?? w.id} later`} disabled={i === board.widgets.length - 1} onClick={() => shift(i, 1)}>
                        <ArrowRight aria-hidden size={14} strokeWidth={1.5} />
                      </Button>
                      <Select aria-label={`Width of ${def?.title ?? w.id}`} value={w.width} onChange={(e) => edit((all) => all.map((x, j) => (j === i ? { ...x, width: Number(e.target.value) } : x)))}>
                        {(def?.widths.map((v) => WIDTHS.find((x) => x.value === v)!) ?? WIDTHS).concat(WIDTHS.some((x) => x.value === w.width) ? [] : [{ value: w.width, label: `${w.width} columns` }]).map((x) => (
                          <option key={x.value} value={x.value}>
                            {x.label}
                          </option>
                        ))}
                      </Select>
                      {def && def.settings.length > 0 && (
                        <Button size="sm" variant="ghost" aria-label={`Settings of ${def.title}`} onClick={() => {
                            setSettingsFor(keys[i]);
                            showSettings(keys[i]);
                          }}>
                          <Settings2 aria-hidden size={14} strokeWidth={1.5} />
                        </Button>
                      )}
                      <Button size="sm" variant="ghost" aria-label={`Remove ${def?.title ?? w.id}`} onClick={() => drop(i)}>
                        <X aria-hidden size={14} strokeWidth={1.5} />
                      </Button>
                    </div>
                  )}
                  {View ? (
                    <WidgetBoundary title={def!.title}>
                      <View settings={w.settings ?? {}} />
                    </WidgetBoundary>
                  ) : <Unavailable id={w.id} />}
                </div>
              );
            })}
          </div>
          </FleetFeed>
        )}
        <p role="status" className="sr-only">
          {said || (customizing ? "Customizing the dashboard" : "")}
        </p>
      </div>
    </div>
  );
}
