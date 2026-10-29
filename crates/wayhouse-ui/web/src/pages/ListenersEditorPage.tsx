import { useMemo } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Plus, Trash2 } from "lucide-react";
import { ConfigSaveBar } from "../components/config/ConfigSaveBar";
import { PatchLines } from "../components/editors/PatchLines";
import { RouteEditor } from "../components/editors/RouteEditor";
import { SchemaForm } from "../components/form/SchemaForm";
import { AlertBar, PageHeader } from "../components/shell/Page";
import { EmptyState, ErrorState, Loading } from "../components/shell/States";
import { Button } from "../components/ui/Button";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { Switch } from "../components/ui/Switch";
import type { State } from "../components/ui/StatusMark";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { useSharedConfigDoc } from "../lib/config/SharedConfigDoc";
import { newListener, type Json } from "../lib/editors/listeners";
import { set } from "../lib/form/path";
import { useFormLayout } from "../lib/form/useFormLayout";
import { useCan } from "../lib/session";
import { useFleetSocket } from "../useFleetSocket";
import { useState } from "react";

const names = (list: unknown): string[] => (Array.isArray(list) ? list.map((x: Json) => x?.name).filter((n): n is string => typeof n === "string") : []);

/** Configure > Listeners and routes: the `listeners:` part of the configuration. */
export function ListenersEditorPage() {
  const doc = useSharedConfigDoc();
  const { layout, notice } = useFormLayout(doc.schema);
  const canWrite = useCan("admin");
  const { confirm, dialog } = useConfirm();
  const { instances } = useFleetSocket();
  const [params, setParams] = useSearchParams();
  const [advanced, setAdvanced] = useState(false);

  const root = (doc.value ?? {}) as Json;
  const listeners: Json[] = Array.isArray(root.listeners) ? root.listeners : [];
  const pools = names(root.pools);
  const resolvers = names(root.resolvers);
  const index = Math.max(0, listeners.findIndex((l) => l.name === params.get("name")));
  const current = listeners[index];

  /** Live pool state from the fleet socket: every backend of the pool, over every instance that reports it. */
  const stateOf = useMemo(() => {
    const backends = new Map<string, boolean[]>();
    for (const inst of instances) for (const p of inst.pools) backends.set(p.name, [...(backends.get(p.name) ?? []), ...p.backends.map((b) => b.healthy)]);
    return (kind: "pool" | "resolver", name: string): State => {
      const b = kind === "pool" ? backends.get(name) : undefined;
      if (!b || b.length === 0) return "unknown";
      return b.every(Boolean) ? "up" : b.some(Boolean) ? "degraded" : "down";
    };
  }, [instances]);

  if (doc.loading) return <Loading />;
  if (doc.error || !doc.schema) return <ErrorState onRetry={doc.reload}>{doc.error ?? "The configuration schema is not available."}</ErrorState>;

  const put = (path: (string | number)[], next: unknown) => doc.setValue(set(doc.value ?? {}, path, next));
  const select = (name: string) => setParams({ name }, { replace: true });

  return (
    <div className="max-w-4xl">
      <PageHeader
        title="Listeners and routes"
        summary="Where the proxy listens and how each connection is routed to a pool or resolver."
        actions={
          <label className="flex items-center gap-2 text-sm text-ink-muted">
            <Switch checked={advanced} onCheckedChange={setAdvanced} aria-label="Show advanced" />
            Show advanced
          </label>
        }
      />
      {notice && <AlertBar>{notice}</AlertBar>}
      {doc.parseError && <AlertBar>The YAML on the Source page does not parse, so the editors are off until it does: {doc.parseError}</AlertBar>}

      {!doc.parseError && (
        <>
          {listeners.length === 0 ? (
            <EmptyState>No listener is configured. Add one to start accepting connections.</EmptyState>
          ) : (
            <Table aria-label="Listeners">
              <THead>
                <TR>
                  <TH>Name</TH>
                  <TH>Bind</TH>
                  <TH>Protocol</TH>
                  <TH>Pool</TH>
                  <TH numeric>Routes</TH>
                </TR>
              </THead>
              <TBody>
                {listeners.map((l, i) => (
                  <TR key={i} selected={i === index}>
                    <TD className="font-mono">
                      <Link replace className="underline" aria-current={i === index || undefined} to={`?${new URLSearchParams({ name: String(l.name ?? "") })}`}>
                        {String(l.name ?? "(unnamed)")}
                      </Link>
                    </TD>
                    <TD className="font-mono">{String(l.bind ?? "")}</TD>
                    <TD>{String(l.protocol ?? "")}</TD>
                    <TD className="font-mono">{l.pool ? String(l.pool) : "–"}</TD>
                    <TD numeric>{Array.isArray(l.routes) ? l.routes.length : 0}</TD>
                  </TR>
                ))}
              </TBody>
            </Table>
          )}
          <div className="my-3 flex gap-2">
            <Button
              disabled={!canWrite}
              onClick={() => {
                const created = newListener(listeners);
                put(["listeners"], [...listeners, created]);
                select(created.name);
              }}
            >
              <Plus aria-hidden size={14} strokeWidth={1.5} /> Add listener
            </Button>
          </div>

          {current && (
            <section aria-label={`Listener ${current.name}`} className="mt-4">
              <div className="mb-2 flex items-center justify-between gap-2">
                <h2 className="font-mono text-lg font-semibold">{String(current.name)}</h2>
                <Button
                  variant="danger"
                  disabled={!canWrite}
                  onClick={async () => {
                    const ok = await confirm({
                      title: `Remove listener ${current.name}?`,
                      description: "It stops accepting connections once the change is applied. Nothing changes until you apply.",
                      confirmLabel: "Remove",
                    });
                    if (ok) put(["listeners"], listeners.filter((_, i) => i !== index));
                  }}
                >
                  <Trash2 aria-hidden size={14} strokeWidth={1.5} /> Remove listener
                </Button>
              </div>
              <PatchLines listener={current as any} stateOf={stateOf} />
              <SchemaForm schema={doc.schema} value={doc.value} saved={doc.savedValue} onChange={doc.setValue} layout={layout} path={["listeners", index]} page="listener" showAdvanced={advanced} readonly={!canWrite} />
              <RouteEditor routes={Array.isArray(current.routes) ? current.routes : []} pools={pools} resolvers={resolvers} readOnly={!canWrite} onChange={(routes) => put(["listeners", index, "routes"], routes.length ? routes : undefined)} />
            </section>
          )}
        </>
      )}
      <ConfigSaveBar doc={doc} />
      {dialog}
    </div>
  );
}
