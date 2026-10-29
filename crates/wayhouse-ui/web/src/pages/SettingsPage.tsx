import { useEffect, useState } from "react";
import { ApiError, getCurrentConfig, submitConfig } from "../api";
import { parseConfigText, stringifyConfigDoc, type ConfigDoc } from "../lib/configDoc";
import { Button, Input } from "../components/ui/Button";
import { Select, Textarea } from "../components/ui/Input";
import { Section } from "../components/ui/Section";
import { AlertBar, PageHeader } from "../components/shell/Page";
import { Loading } from "../components/shell/States";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { useCan } from "../lib/session";

export function SettingsPage() {
  const [text, setText] = useState("");
  const [revision, setRevision] = useState<string | null>(null);
  const [doc, setDoc] = useState<ConfigDoc | null>(null);
  const [parseError, setParseError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
    const { confirm, dialog } = useConfirm();
  const canApply = useCan("admin");

  async function refresh() {
    try {
      const current = await getCurrentConfig().catch((err) => {
        if (err instanceof ApiError && err.status === 404) return { text: "", revision: null };
        throw err;
      });
      setText(current.text);
      setRevision(current.revision);
      const { doc, error } = parseConfigText(current.text);
      setDoc(doc);
      setParseError(error);
    } catch (err) {
      setNotice(err instanceof ApiError ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function onRawChange(next: string) {
    setText(next);
    const { doc, error } = parseConfigText(next);
    setDoc(doc);
    setParseError(error);
  }

  function patchDoc(patch: (draft: ConfigDoc) => void) {
    if (!doc) return;
    const next: ConfigDoc = structuredClone(doc);
    patch(next);
    setDoc(next);
    setText(stringifyConfigDoc(next));
  }

  async function submit() {
    const ok = await confirm({
      title: "Apply this configuration?",
      description:
        "This submits a new revision that is pushed to every instance. A bad config can disrupt live traffic; you can roll back from Config history.",
      confirmLabel: "Apply",
    });
    if (!ok) return;
    try {
      const result = await submitConfig(text);
      setNotice(`accepted as revision ${result.revision}`);
      await refresh();
    } catch (err) {
      setNotice(err instanceof ApiError ? err.message : String(err));
    }
  }

  if (loading) return <Loading />;

  return (
    <div className="max-w-3xl">
      <PageHeader
        title="Settings"
        summary={revision ? `Revision ${revision}` : "No revision submitted yet"}
      />

      {notice && (
        <p role="status" className="mb-4 text-sm text-ink-muted">{notice}</p>
      )}

      {parseError ? (
        <AlertBar>
          The raw YAML below has a parse error, so the structured form is disabled until it's fixed: {parseError}
        </AlertBar>
      ) : (
        doc && <StructuredForm doc={doc} patch={patchDoc} />
      )}

      <Section title="Advanced: raw YAML" advanced className="mt-6">
        <p className="mb-2 max-w-(--measure) text-xs text-ink-muted">
          Everything here is also reachable above, but every field, including ones with no form yet, is always
          editable here.
        </p>
        <Textarea
          value={text}
          onChange={(e) => onRawChange(e.target.value)}
          spellCheck={false}
          rows={22}
          className="w-full text-xs"
        />
      </Section>

      <div className="sticky bottom-0 mt-4 flex justify-end border-t border-line bg-bg py-2">
        <Button variant="primary" onClick={submit} disabled={!!parseError || !canApply}
          title={canApply ? undefined : "Changing the configuration needs the admin role"}
        >
          Submit as a new revision
        </Button>
      </div>
      {dialog}
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="flex flex-col gap-1 text-xs text-ink-muted">
      {label}
      {children}
    </label>
  );
}

function StructuredForm({
  doc,
  patch,
}: {
  doc: ConfigDoc;
  patch: (fn: (draft: ConfigDoc) => void) => void;
}) {
  const s = doc.settings ?? {};

  return (
    <div className="flex flex-col gap-4">
      <Section title="General">
        <div className="grid gap-3 min-[760px]:grid-cols-3">
          <Field label="workers (0 = one per CPU)">
            <Input
              type="number"
              value={s.workers ?? 0}
              onChange={(e) => patch((d) => (d.settings.workers = Number(e.target.value)))}
            />
          </Field>
          <Field label="shutdown grace (sec)">
            <Input
              type="number"
              value={s.shutdown_grace_sec ?? ""}
              onChange={(e) => patch((d) => (d.settings.shutdown_grace_sec = Number(e.target.value)))}
            />
          </Field>
          <Field label="admin listen">
            <Input
              value={s.admin?.listen ?? ""}
              onChange={(e) => patch((d) => (d.settings.admin = { ...d.settings.admin, listen: e.target.value }))}
            />
          </Field>
          <Field label="admin auth token">
            <Input
              type="password"
              value={s.admin?.auth_token ?? ""}
              onChange={(e) =>
                patch((d) => (d.settings.admin = { ...d.settings.admin, auth_token: e.target.value }))
              }
            />
          </Field>
          <Field label="geo DB path">
            <Input
              value={s.geo_db ?? ""}
              onChange={(e) => patch((d) => (d.settings.geo_db = e.target.value || undefined))}
            />
          </Field>
          <Field label="fleet group (e.g. eu/frankfurt/cluster-a)">
            <Input
              value={s.group ?? ""}
              onChange={(e) => patch((d) => (d.settings.group = e.target.value || undefined))}
            />
          </Field>
        </div>
      </Section>

      <Section title="Global limits">
        <div className="grid gap-3 min-[760px]:grid-cols-3">
          <Field label="max connections">
            <Input
              type="number"
              value={s.limits?.max_connections ?? ""}
              onChange={(e) =>
                patch(
                  (d) =>
                    (d.settings.limits = {
                      ...d.settings.limits,
                      max_connections: e.target.value ? Number(e.target.value) : undefined,
                    }),
                )
              }
            />
          </Field>
          <Field label="max UDP sessions">
            <Input
              type="number"
              value={s.limits?.max_udp_sessions ?? ""}
              onChange={(e) =>
                patch(
                  (d) =>
                    (d.settings.limits = {
                      ...d.settings.limits,
                      max_udp_sessions: e.target.value ? Number(e.target.value) : undefined,
                    }),
                )
              }
            />
          </Field>
          <Field label="max new sessions/sec">
            <Input
              type="number"
              value={s.limits?.max_new_sessions_per_sec ?? ""}
              onChange={(e) =>
                patch(
                  (d) =>
                    (d.settings.limits = {
                      ...d.settings.limits,
                      max_new_sessions_per_sec: e.target.value ? Number(e.target.value) : undefined,
                    }),
                )
              }
            />
          </Field>
        </div>
      </Section>

      <Section title="Pools">
        <div className="flex flex-col gap-3">
          {(doc.pools ?? []).map((pool, i) => (
            <div key={i} className="flex flex-wrap items-center gap-2 border border-line p-2">
              <Input
                placeholder="name"
                value={pool.name}
                onChange={(e) => patch((d) => (d.pools[i].name = e.target.value))}
              />
              <Input
                placeholder="balancer (round_robin, ...)"
                value={pool.balancer ?? ""}
                onChange={(e) => patch((d) => (d.pools[i].balancer = e.target.value || undefined))}
              />
              <Input
                placeholder="targets, comma-separated (ip:port, ...)"
                className="min-w-64 flex-1"
                value={(pool.targets ?? []).join(", ")}
                onChange={(e) =>
                  patch(
                    (d) =>
                      (d.pools[i].targets = e.target.value
                        .split(",")
                        .map((s) => s.trim())
                        .filter(Boolean)),
                  )
                }
              />
              <Button variant="danger" size="sm" onClick={() => patch((d) => d.pools.splice(i, 1))}>remove</Button>
            </div>
          ))}
          <Button
            variant="ghost"
            className="self-start"
            onClick={() => patch((d) => d.pools.push({ name: "", targets: [] }))}
          >
            + add pool
          </Button>
        </div>
      </Section>

      <Section title="Listeners">
        <div className="flex flex-col gap-3">
          {(doc.listeners ?? []).map((l, i) => (
            <div key={i} className="flex flex-wrap items-center gap-2 border border-line p-2">
              <Input
                placeholder="name"
                value={l.name}
                onChange={(e) => patch((d) => (d.listeners[i].name = e.target.value))}
              />
              <Input
                placeholder="bind (host:port)"
                value={l.bind}
                onChange={(e) => patch((d) => (d.listeners[i].bind = e.target.value))}
              />
              <Select
                value={l.protocol ?? "tcp"}
                onChange={(e) => patch((d) => (d.listeners[i].protocol = e.target.value))}
                >
                <option value="tcp">tcp</option>
                <option value="udp">udp</option>
              </Select>
              <Input
                placeholder="pool"
                value={l.pool ?? ""}
                onChange={(e) => patch((d) => (d.listeners[i].pool = e.target.value || undefined))}
              />
              <Button variant="danger" size="sm" onClick={() => patch((d) => d.listeners.splice(i, 1))}>remove</Button>
              <span className="w-full text-xs text-ink-muted">
                Routes/matchers on this listener aren't editable here yet — use the raw YAML panel below.
              </span>
            </div>
          ))}
          <Button
            variant="ghost"
            className="self-start"
            onClick={() => patch((d) => d.listeners.push({ name: "", bind: "" }))}
          >
            + add listener
          </Button>
        </div>
      </Section>

      <Section title="Sniffers">
        <Field label="sniffers.dir">
          <Input
            value={s.sniffers?.dir ?? ""}
            onChange={(e) =>
              patch((d) => {
                const dir = e.target.value;
                d.settings.sniffers = dir ? { ...d.settings.sniffers, dir } : undefined;
              })
            }
            className="max-w-md"
          />
        </Field>
        <p className="mt-2 text-xs text-ink-muted">
          Manage which modules live in this directory from the Sniffers page.
        </p>
      </Section>

      <Section title="Tier-2 regional health fabric">
        <div className="grid gap-3 min-[760px]:grid-cols-2">
          <Field label="failure domain">
            <Input
              value={s.failure_domain ?? ""}
              onChange={(e) => patch((d) => (d.settings.failure_domain = e.target.value || undefined))}
            />
          </Field>
          <Field label="gossip bind">
            <Input
              value={s.gossip?.bind ?? ""}
              onChange={(e) => patch((d) => (d.settings.gossip = { ...d.settings.gossip, bind: e.target.value }))}
            />
          </Field>
          <Field label="gossip seeds, comma-separated">
            <Input
              value={(s.gossip?.seeds ?? []).join(", ")}
              onChange={(e) =>
                patch(
                  (d) =>
                    (d.settings.gossip = {
                      ...d.settings.gossip,
                      seeds: e.target.value
                        .split(",")
                        .map((s) => s.trim())
                        .filter(Boolean),
                    }),
                )
              }
            />
          </Field>
          <Field label="quorum fraction (>0.5, <=1.0)">
            <Input
              type="number"
              step="0.01"
              value={s.gossip?.quorum_fraction ?? ""}
              onChange={(e) =>
                patch(
                  (d) =>
                    (d.settings.gossip = {
                      ...d.settings.gossip,
                      quorum_fraction: Number(e.target.value),
                    }),
                )
              }
            />
          </Field>
          <Field label="gossip psk">
            <Input
              type="password"
              value={s.gossip?.psk ?? ""}
              onChange={(e) => patch((d) => (d.settings.gossip = { ...d.settings.gossip, psk: e.target.value }))}
            />
          </Field>
        </div>
        <p className="mt-2 text-xs text-ink-muted">
          Both fields must be set together, or neither — see docs/05-configuration.md.
        </p>
      </Section>
    </div>
  );
}
