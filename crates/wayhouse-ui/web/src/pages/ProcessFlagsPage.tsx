import { useCallback, useEffect, useMemo, useState } from "react";
import { ApiError, applyProcess, confirmProcess, listProcess, undoProcess } from "../api";
import type { ProcessFlag, ProcessList, ProcessProgram, ProcessValue } from "../types";
import { PageHeader } from "../components/shell/Page";
import { EmptyState, ErrorState, Loading } from "../components/shell/States";
import { Button } from "../components/ui/Button";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { Input, Textarea } from "../components/ui/Input";
import { Section } from "../components/ui/Section";
import { Switch } from "../components/ui/Switch";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { useCan } from "../lib/session";

const errorText = (err: unknown) => (err instanceof ApiError ? err.message : String(err));

type Draft = Record<string, ProcessValue>;

/** A value is "unset" when it is empty text, an empty list or `false`. */
const isSet = (v: ProcessValue | undefined): v is ProcessValue =>
  v !== undefined && v !== false && v !== "" && !(Array.isArray(v) && v.length === 0);

/** The draft without unset entries, so "cleared" and "never set" compare equal. */
const compact = (d: Draft): Draft => Object.fromEntries(Object.entries(d).filter(([, v]) => isSet(v)));

// The command-line flags of the fleet's own programs, written to a settings file each program
// reads at start. Applying is a trial: the program restarts with the change and goes back unless
// it is confirmed here.
export function ProcessFlagsPage() {
  const canWrite = useCan("admin");
  const [list, setList] = useState<ProcessList | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setList(await listProcess());
      setError(null);
    } catch (err) {
      setError(errorText(err));
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);

  // While a change waits for confirmation the program restarts, confirms or goes back by itself,
  // so look again every few seconds.
  const waiting = list?.programs.some((p) => p.file?.pending) ?? false;
  useEffect(() => {
    if (!waiting) return;
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, [waiting, load]);

  return (
    <>
      <PageHeader
        title="Process flags"
        summary="The command-line flags of the proxy, controller, aggregator and UI. A change restarts the program and goes back by itself unless you confirm it."
      />
      {error && <ErrorState onRetry={load}>{error}</ErrorState>}
      {!list && !error && <Loading />}
      {list && !list.dir_configured && (
        <EmptyState>
          This UI was started without <code className="font-mono">--settings-dir</code>. Start the programs and the UI with the same
          settings directory (see docs/12-deployment.md) and their flags appear here.
        </EmptyState>
      )}
      {list?.dir_configured && (
        <div className="flex flex-col">
          {list.programs.map((p) => (
            <ProgramSection key={p.program} program={p} canWrite={canWrite} onChanged={load} />
          ))}
        </div>
      )}
      <p className="mt-2 text-sm text-ink-muted">
        Tokens, passwords and destinations (URLs, files), safety overrides such as <code className="font-mono">--insecure-no-auth</code> are not editable here; set
        them on the command line or in the service unit. Values you leave empty fall back to the program's default.
      </p>
    </>
  );
}

function ProgramSection({ program, canWrite, onChanged }: { program: ProcessProgram; canWrite: boolean; onChanged: () => void }) {
  const file = program.file;
  const saved = useMemo<Draft>(() => file?.values ?? {}, [file]);
  const [draft, setDraft] = useState<Draft>(saved);
  const [filter, setFilter] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { confirm, dialog } = useConfirm();

  // A newer file (a settled trial, an undo) replaces what was being typed.
  useEffect(() => {
    setDraft(saved);
  }, [saved, file?.rev]);

  const editable = program.flags.filter((f) => f.editable);
  const locked = program.flags.filter((f) => !f.editable);
  const shown = editable.filter((f) => f.name.includes(filter.trim().toLowerCase()));
  const changed = Object.keys({ ...compact(draft), ...compact(saved) }).filter(
    (k) => JSON.stringify(compact(draft)[k]) !== JSON.stringify(compact(saved)[k]),
  ).length;
  const pending = file?.pending ?? null;

  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      onChanged();
    } catch (err) {
      setError(errorText(err));
      if (err instanceof ApiError && err.status === 409) onChanged();
    } finally {
      setBusy(false);
    }
  }

  async function apply() {
    const ok = await confirm({
      title: `Restart the ${program.label.toLowerCase()} with ${changed} changed ${changed === 1 ? "flag" : "flags"}?`,
      description:
        "It restarts now and may be unreachable for a moment. If you do not confirm within 60 seconds of its start, or it cannot start, it goes back to its previous flags.",
      confirmLabel: "Restart with the change",
      danger: false,
    });
    if (ok) await run(() => applyProcess(program.program, compact(draft), file?.rev ?? 0));
  }

  const set = (name: string, v: ProcessValue) => setDraft((d) => ({ ...d, [name]: v }));

  return (
    <Section title={program.label} changed={changed} defaultOpen={program.connected}>
      {!program.connected ? (
        <EmptyState>
          Not connected. It publishes its flags when it starts with <code className="font-mono">--settings-dir</code> pointing at this
          directory.
        </EmptyState>
      ) : (
        <div className="flex flex-col gap-3 pb-3">
          {file?.error && (
            <p role="alert" className="text-sm text-bad">
              The program refused the last change and went back: {file.error}
            </p>
          )}
          {pending && (
            <div role="status" className="flex flex-wrap items-center gap-2 border border-line bg-surface px-3 py-2 text-sm">
              <span className="flex-1">
                A change is on trial. The program went back to its previous flags if it is not confirmed within {pending.confirm_secs}{" "}
                seconds of its start. Check that you can still reach everything, then confirm.
              </span>
              <Button variant="primary" disabled={!canWrite || busy} onClick={() => run(() => confirmProcess(program.program))}>
                Confirm
              </Button>
              <Button disabled={!canWrite || busy} onClick={() => run(() => undoProcess(program.program))}>
                Undo
              </Button>
            </div>
          )}
          <label className="flex max-w-sm flex-col gap-1 text-sm text-ink-muted">
            Filter flags
            <Input value={filter} onChange={(e) => setFilter(e.target.value)} aria-label={`Filter ${program.label} flags`} />
          </label>
          <Table>
            <THead>
              <TR>
                <TH>flag</TH>
                <TH>value</TH>
                <TH>default and meaning</TH>
              </TR>
            </THead>
            <TBody>
              {shown.map((f) => (
                <TR key={f.name}>
                  <TD className="align-top font-mono text-ink">--{f.name}</TD>
                  <TD className="align-top">
                    <FlagInput
                      flag={f}
                      value={draft[f.name]}
                      disabled={!canWrite || busy || pending !== null}
                      onChange={(v) => set(f.name, v)}
                    />
                  </TD>
                  <TD className="align-top text-sm text-ink-muted">
                    {f.default !== null && <span className="mr-2 font-mono">default {f.default}</span>}
                    {f.help}
                  </TD>
                </TR>
              ))}
            </TBody>
          </Table>
          {locked.length > 0 && (
            <p className="text-sm text-ink-muted">
              Not editable here: {locked.map((f) => `--${f.name}`).join(", ")}.
            </p>
          )}
          {error && (
            <p role="alert" className="text-sm text-bad">
              {error}
            </p>
          )}
          <div className="flex gap-2">
            <Button variant="primary" disabled={!canWrite || busy || pending !== null || changed === 0} onClick={apply}>
              Apply as a trial
            </Button>
            <Button variant="ghost" disabled={changed === 0 || busy} onClick={() => setDraft(saved)}>
              Discard
            </Button>
          </div>
          {dialog}
        </div>
      )}
    </Section>
  );
}

function FlagInput({
  flag,
  value,
  disabled,
  onChange,
}: {
  flag: ProcessFlag;
  value: ProcessValue | undefined;
  disabled: boolean;
  onChange: (v: ProcessValue) => void;
}) {
  const label = `--${flag.name}`;
  if (flag.kind === "bool") {
    return <Switch aria-label={label} checked={value === true} disabled={disabled} onCheckedChange={onChange} />;
  }
  if (flag.kind === "list") {
    return <ListInput label={label} value={value} disabled={disabled} onChange={onChange} />;
  }
  return (
    <Input
      mono
      aria-label={label}
      disabled={disabled}
      placeholder={flag.value_name ?? ""}
      value={typeof value === "string" ? value : ""}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}

/** One entry per line. Keeps the raw text while typing, so a new empty line can be started. */
function ListInput({
  label,
  value,
  disabled,
  onChange,
}: {
  label: string;
  value: ProcessValue | undefined;
  disabled: boolean;
  onChange: (v: ProcessValue) => void;
}) {
  const lines = Array.isArray(value) ? value : typeof value === "string" ? [value] : [];
  const [text, setText] = useState(lines.join("\n"));
  const parse = (t: string) => t.split("\n").map((l) => l.trim()).filter((l) => l !== "");
  // Take over a value that changed from outside (a discard, a newer file).
  useEffect(() => {
    setText((t) => (JSON.stringify(parse(t)) === JSON.stringify(lines) ? t : lines.join("\n")));
  }, [JSON.stringify(lines)]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <Textarea
      aria-label={label}
      rows={Math.max(2, text.split("\n").length)}
      disabled={disabled}
      value={text}
      placeholder="one per line"
      onChange={(e) => {
        setText(e.target.value);
        onChange(parse(e.target.value));
      }}
    />
  );
}
