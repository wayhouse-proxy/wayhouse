import { useState } from "react";
import { ApiError, deleteBackend, drainInstance, patchBackend, undrainInstance } from "../../api";
import type { FleetNode } from "../../lib/fleet/nodes";
import { useCan } from "../../lib/session";
import type { FleetInstanceView } from "../../types";
import { Badge } from "../ui/Badge";
import { Button } from "../ui/Button";
import { useConfirm } from "../ui/ConfirmDialog";
import { Select } from "../ui/Input";
import { useToast } from "../ui/Toast";
import { NodeStateBadge } from "./NodeStateBadge";

type BackendState = "enabled" | "draining" | "disabled";

/**
 * What the inspector shows for one node: its details, its tags (each one a filter), drain and undrain,
 * and, from the live socket, its pools with backend state and remove. `node` is the page row (absent when
 * the node is not on this page), `live` the socket's view of the same instance.
 */
export function NodeInspector({
  instance,
  node,
  live,
  onTag,
}: {
  instance: string;
  node?: FleetNode;
  live?: FleetInstanceView;
  onTag: (tag: string) => void;
}) {
  const canOperate = useCan("operator");
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const [busy, setBusy] = useState(false);
  const off = busy || !canOperate;

  function run(label: string, p: Promise<unknown>) {
    setBusy(true);
    p.then(() => toast(`${label}: done`)).catch((e) => toast(`${label} failed: ${e instanceof ApiError ? e.message : e}`, "down")).finally(() => setBusy(false));
  }

  const pools = live?.pools ?? [];
  return (
    <div className="flex flex-col gap-4 text-sm">
      {node ? (
        <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1">
          <dt className="text-ink-muted">State</dt>
          <dd>
            <NodeStateBadge state={node.state} />
          </dd>
          <dt className="text-ink-muted">Region</dt>
          <dd>{node.region ?? "none"}</dd>
          <dt className="text-ink-muted">Group</dt>
          <dd className="font-mono">{node.group ?? "none"}</dd>
          <dt className="text-ink-muted">Version</dt>
          <dd className="font-mono">{`v${node.version || "?"} · protocol ${node.protocol || "?"}`}</dd>
          {node.skew !== "none" && (
            <>
              <dt className="text-ink-muted">Skew</dt>
              <dd>{node.skew === "within-window" ? <Badge tone="warn">older, in window</Badge> : <Badge tone="bad">outside window</Badge>}</dd>
            </>
          )}
          <dt className="text-ink-muted">Sessions</dt>
          <dd className="tabular-nums">{`${node.sessions.tcp} TCP · ${node.sessions.udp} UDP`}</dd>
          <dt className="text-ink-muted">Backends</dt>
          <dd className="tabular-nums">{`${node.backends}, ${node.unhealthy_backends} unhealthy`}</dd>
          <dt className="text-ink-muted">Last seen</dt>
          <dd className="tabular-nums">{(node.last_seen_ms_ago / 1000).toFixed(1)} s ago</dd>
        </dl>
      ) : (
        <p className="text-ink-muted">This node is not on the current page of results.</p>
      )}

      <section aria-label="Tags">
        <h3 className="mb-1 font-semibold">Tags</h3>
        {node && Object.keys(node.tags).length > 0 ? (
          <ul className="flex flex-wrap gap-1">
            {Object.entries(node.tags).map(([k, v]) => (
              <li key={k}>
                <button type="button" title="Filter by this tag" onClick={() => onTag(`${k}=${v}`)} className="rounded-sm border border-line px-1.5 py-0.5 font-mono text-xs hover:bg-surface-sunken">
                  {k}={v}
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-ink-muted">None. A node reports its tags itself (<code className="font-mono">--tag key=value</code>).</p>
        )}
      </section>

      <section aria-label="Actions" className="flex gap-2">
        <Button
          disabled={off}
          onClick={async () => {
            const ok = await confirm({
              title: `Drain ${instance}?`,
              description: "It stops accepting new sessions; existing ones finish. Undrain reverses this.",
              confirmLabel: "Drain",
            });
            if (ok) run(`drain ${instance}`, drainInstance(instance));
          }}
        >
          Drain
        </Button>
        <Button disabled={off} onClick={() => run(`undrain ${instance}`, undrainInstance(instance))}>
          Undrain
        </Button>
      </section>

      <section aria-label="Pools">
        <h3 className="mb-1 font-semibold">Pools</h3>
        {pools.length === 0 ? (
          <p className="text-ink-muted">No pools reported.</p>
        ) : (
          <>
            <p className="mb-2 text-xs text-ink-muted">Backend state and remove apply fleet-wide: every instance with the same pool and address applies the change.</p>
            {pools.map((pool) => (
              <div key={pool.name} className="mb-2">
                <p className="font-mono text-xs text-ink-muted">{pool.name}</p>
                <ul>
                  {pool.backends.map((b) => (
                    <li key={b.addr} className="flex flex-wrap items-center gap-2 border-t border-line py-1">
                      <span className="font-mono">{b.addr}</span>
                      <Badge tone={b.healthy ? "good" : "bad"}>{b.healthy ? "healthy" : "unhealthy"}</Badge>
                      <span className="text-xs text-ink-muted tabular-nums">{b.active} active</span>
                      <Select
                        aria-label={`State of ${b.addr}`}
                        value={b.state}
                        disabled={off}
                        onChange={async (e) => {
                          const next = e.target.value as BackendState;
                          // Taking a backend out of rotation is the risky direction; returning it to enabled is restorative.
                          if (
                            next !== "enabled" &&
                            !(await confirm({
                              title: `Set ${b.addr} to ${next}?`,
                              description: `Applies to every instance that has pool ${pool.name} and this address, not just ${instance}.`,
                              confirmLabel: next === "disabled" ? "Disable" : "Set draining",
                            }))
                          )
                            return;
                          run(`patch ${pool.name}/${b.addr}`, patchBackend(pool.name, b.addr, next));
                        }}
                      >
                        <option value="enabled">enabled</option>
                        <option value="draining">draining</option>
                        <option value="disabled">disabled</option>
                      </Select>
                      <Button
                        variant="danger"
                        size="sm"
                        disabled={off}
                        onClick={async () => {
                          const ok = await confirm({
                            title: `Remove ${b.addr} from ${pool.name}?`,
                            description: `Removes it from every instance that has this pool and address, not just ${instance}. Live sessions to it end.`,
                            confirmLabel: "Remove",
                          });
                          if (ok) run(`remove ${pool.name}/${b.addr}`, deleteBackend(pool.name, b.addr));
                        }}
                      >
                        remove
                      </Button>
                    </li>
                  ))}
                </ul>
              </div>
            ))}
          </>
        )}
      </section>
      {dialog}
    </div>
  );
}
