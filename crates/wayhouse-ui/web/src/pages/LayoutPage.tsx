import { useEffect, useMemo, useRef, useState } from "react";
import { SchemaForm } from "../components/form/SchemaForm";
import { ErrorState, Loading } from "../components/shell/States";
import { AlertBar, PageHeader } from "../components/shell/Page";
import { Button } from "../components/ui/Button";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { FieldRow } from "../components/ui/FieldRow";
import { Select, Textarea } from "../components/ui/Input";
import { Section } from "../components/ui/Section";
import { useConfigDoc } from "../lib/config/useConfigDoc";
import { DEFAULT_LAYOUT } from "../lib/form/layout";
import { useLayout } from "../lib/form/useLayout";
import { useCan } from "../lib/session";
import { useLayoutOverride, useResetLayout, useSaveLayout } from "../lib/uidata";

const EXAMPLE = `{
  "version": 1,
  "fields": {}
}`;
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** Settings > Layout: the JSON override of the shipped form layout (decision 5B; docs/ui-form-layout.md). */
export function LayoutPage() {
  const canWrite = useCan("admin");
  const stored = useLayoutOverride();
  const save = useSaveLayout();
  const reset = useResetLayout();
  const doc = useConfigDoc();
  const { confirm, dialog } = useConfirm();
  const [draft, setDraft] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [section, setSection] = useState("");
  const file = useRef<HTMLInputElement>(null);

  const text = draft ?? stored.data ?? "";
  const { layout, notice } = useLayout(doc.schema, text);
  const sections = useMemo(() => Object.keys((doc.schema?.properties as object | undefined) ?? {}), [doc.schema]);
  const shown = section || sections[0] || "";

  useEffect(() => setFailure(null), [draft]);

  if (stored.isPending || doc.loading) return <Loading />;
  if (stored.isError) return <ErrorState onRetry={() => stored.refetch()}>{message(stored.error)}</ErrorState>;

  const dirty = draft !== null && draft !== (stored.data ?? "");
  let syntax: string | null = null;
  if (text.trim()) {
    try {
      JSON.parse(text);
    } catch (e) {
      syntax = message(e);
    }
  }
  const run = (p: Promise<unknown>, done: () => void) => p.then(done).catch((e) => setFailure(message(e)));

  async function onReset() {
    const ok = await confirm({
      title: "Use the shipped layout?",
      description: "This deletes the stored override for every operator of this deployment.",
      confirmLabel: "Reset to default",
    });
    if (ok) run(reset.mutateAsync(), () => setDraft(null));
  }

  async function onImport(f: File | undefined) {
    if (f) setDraft(await f.text());
  }

  function onExport() {
    const url = URL.createObjectURL(new Blob([text || EXAMPLE], { type: "application/json" }));
    const a = Object.assign(document.createElement("a"), { href: url, download: "form-layout.json" });
    a.click();
    URL.revokeObjectURL(url);
  }

  return (
    <div className="max-w-5xl">
      {dialog}
      <PageHeader title="Layout" summary="Reorder, group or hide form fields for every operator of this deployment." />
      {!canWrite && <AlertBar>Only admins can change the layout. You can read it and try edits in the preview.</AlertBar>}
      <Section title="Override">
        <p className="mb-2 text-sm text-ink-muted">{stored.data === null ? "No override stored: the forms use the shipped layout." : "Stored override, merged over the shipped layout."}</p>
        <FieldRow
          label="Layout JSON"
          error={syntax ?? failure ?? undefined}
          help='Keys are JSON Pointers into the config, for example "/settings/workers". Each field takes help, group, order, widget, advanced and page.'
          render={(a) => <Textarea {...a} rows={14} spellCheck={false} value={text} placeholder={EXAMPLE} onChange={(e) => setDraft(e.target.value)} />}
        />
        {notice && !syntax && <p role="status" className="text-sm text-warn">{notice}</p>}
        <div className="flex flex-wrap gap-2">
          <Button disabled={!canWrite || !dirty || !!syntax || !text.trim() || save.isPending} onClick={() => run(save.mutateAsync(text), () => setDraft(null))}>
            Save layout
          </Button>
          <Button variant="secondary" disabled={!dirty} onClick={() => setDraft(null)}>
            Discard edits
          </Button>
          <Button variant="secondary" disabled={!canWrite || stored.data === null || reset.isPending} onClick={onReset}>
            Reset to default
          </Button>
          <Button variant="secondary" onClick={() => file.current?.click()}>
            Import JSON
          </Button>
          <Button variant="secondary" onClick={onExport}>
            Export JSON
          </Button>
          <input ref={file} type="file" accept="application/json,.json" hidden aria-label="Import layout JSON" onChange={(e) => onImport(e.target.files?.[0])} />
        </div>
      </Section>
      <Section title="Preview" className="mt-4">
        <p className="mb-2 text-sm text-ink-muted">One real form, laid out with the JSON above. Nothing here is saved to the config.</p>
        {doc.schema && sections.length > 0 ? (
          <>
            <FieldRow
              label="Section"
              render={(a) => (
                <Select {...a} value={shown} onChange={(e) => setSection(e.target.value)}>
                  {sections.map((s) => (
                    <option key={s}>{s}</option>
                  ))}
                </Select>
              )}
            />
            <SchemaForm schema={doc.schema} value={doc.value} saved={doc.savedValue} onChange={() => {}} layout={syntax ? DEFAULT_LAYOUT : layout} path={[shown]} showAdvanced />
          </>
        ) : (
          <p className="text-sm text-ink-muted">{doc.error ?? "The config schema is not available."}</p>
        )}
      </Section>
    </div>
  );
}
