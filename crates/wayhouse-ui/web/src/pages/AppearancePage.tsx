import { useEffect, useRef, useState } from "react";
import type { StoredTheme } from "../api";
import { ThemeEditor } from "../components/appearance/ThemeEditor";
import { ErrorState, Loading } from "../components/shell/States";
import { PageHeader } from "../components/shell/Page";
import { Button } from "../components/ui/Button";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { FieldRow } from "../components/ui/FieldRow";
import { Select } from "../components/ui/Input";
import { Section } from "../components/ui/Section";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { useCan } from "../lib/session";
import { BUILTIN, effective } from "../lib/theme/builtin";
import { useTheme } from "../lib/theme/provider";
import { exportTheme, parseTheme } from "../lib/theme/store";
import { useDeleteTheme, useSaveDefaults, useSaveTheme, useThemes, useUiDefaults } from "../lib/uidata";

const SWATCH = ["bg", "surface", "ink", "select", "good", "warn", "bad", "series-1", "series-2", "series-3"];
const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

/** The theme's colours as a strip. The strip is a hint, the hex values are in the editor, so it is hidden from assistive tech. */
function Swatches({ theme }: { theme: StoredTheme }) {
  const t = effective(theme);
  return (
    <span aria-hidden className="inline-flex overflow-hidden rounded-sm border border-line">
      {SWATCH.map((k) => (
        <span key={k} className="h-4 w-4" style={{ background: t[k] }} />
      ))}
    </span>
  );
}

const builtin = (base: "light" | "dark"): StoredTheme => ({ name: base === "light" ? "Light" : "Dark", base, tokens: {} });

/** Settings > Appearance: this browser's theme, the deployment default, and the custom themes (themes.md). */
export function AppearancePage() {
  const { choice, own, setChoice, preview } = useTheme();
  const canWrite = useCan("admin");
  const themes = useThemes();
  const defaults = useUiDefaults();
  const save = useSaveTheme();
  const remove = useDeleteTheme();
  const saveDefaults = useSaveDefaults();
  const { confirm, dialog } = useConfirm();
  const [draft, setDraft] = useState<{ original?: string; theme: StoredTheme } | null>(null);
  const [previewing, setPreviewing] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const file = useRef<HTMLInputElement>(null);

  useEffect(() => () => preview(null), [preview]);

  if (themes.isPending || defaults.isPending) return <Loading rows={4} />;
  if (themes.error) return <ErrorState onRetry={() => themes.refetch()}>{message(themes.error)}</ErrorState>;
  const saved = themes.data ?? [];
  const defaultTheme = defaults.data?.theme ?? "system";
  const choices = [
    { value: "system", label: "System" },
    { value: "light", label: "Light" },
    { value: "dark", label: "Dark" },
    ...saved.map((t) => ({ value: `custom:${t.name}`, label: t.name })),
  ];
  const defaultChoices = choices.map((c) => ({ value: c.value.replace(/^custom:/, ""), label: c.label }));

  const run = async (fn: () => Promise<unknown>) => {
    setProblem(null);
    try {
      await fn();
    } catch (e) {
      setProblem(message(e));
    }
  };
  const togglePreview = (key: string, t: StoredTheme) => {
    const on = previewing !== key;
    preview(on ? t : null);
    setPreviewing(on ? key : null);
  };
  const download = (t: StoredTheme) => {
    try {
      const url = URL.createObjectURL(new Blob([exportTheme(t)], { type: "application/json" }));
      const a = Object.assign(document.createElement("a"), { href: url, download: `${t.name}.json` });
      a.click();
      URL.revokeObjectURL(url);
    } catch (e) {
      setProblem(`Could not export: ${message(e)}`);
    }
  };
  const importFile = async (f: File | undefined) => {
    if (!f) return;
    setProblem(null);
    try {
      setDraft({ theme: parseTheme(await f.text()) });
    } catch (e) {
      setProblem(message(e));
    }
    if (file.current) file.current.value = "";
  };
  const del = async (t: StoredTheme) => {
    const inUse = defaultTheme === t.name ? " It is the deployment default; the default goes back to System." : "";
    if (!(await confirm({ title: `Delete ${t.name}?`, description: `Browsers using it fall back to System.${inUse}`, confirmLabel: "Delete theme" }))) return;
    if (previewing === t.name) togglePreview(t.name, t);
    await run(() => remove.mutateAsync(t.name));
  };

  const mark = (t: StoredTheme, key: string) => (
    <Button size="sm" variant="ghost" aria-pressed={previewing === key} onClick={() => togglePreview(key, t)}>
      {previewing === key ? "Stop preview" : "Preview"}
    </Button>
  );

  return (
    <div className="max-w-4xl">
      <PageHeader title="Appearance" summary="Pick how the console looks in this browser. Admins also set the deployment default and manage custom themes." />
      {problem && <ErrorState>{problem}</ErrorState>}

      <Section title="Theme">
        <FieldRow
          label="This browser"
          help="Remembered in this browser only."
          render={(a) => (
            <Select {...a} value={own ? choice : "default"} onChange={(e) => setChoice(e.target.value === "default" ? null : e.target.value)}>
              <option value="default">Deployment default ({defaultChoices.find((c) => c.value === defaultTheme)?.label ?? "System"})</option>
              {choices.map((c) => (
                <option key={c.value} value={c.value}>
                  {c.label}
                </option>
              ))}
            </Select>
          )}
        />
        <FieldRow
          label="Deployment default"
          help="What a browser shows until its person picks one."
          render={(a) => (
            <div className="flex items-center gap-2">
              <Select
                {...a}
                value={defaultTheme}
                disabled={!canWrite || saveDefaults.isPending}
                title={canWrite ? undefined : "Changing the default needs the admin role"}
                onChange={(e) => run(() => saveDefaults.mutateAsync({ ...defaults.data, theme: e.target.value === "system" ? undefined : e.target.value }))}
              >
                {defaultChoices.map((c) => (
                  <option key={c.value} value={c.value}>
                    {c.label}
                  </option>
                ))}
              </Select>
            </div>
          )}
        />
      </Section>

      <Section title="Themes" className="mt-4">
        <div className="mb-2 flex justify-end gap-2">
          <input ref={file} type="file" accept="application/json,.json" aria-label="Import a theme file" className="hidden" onChange={(e) => importFile(e.target.files?.[0])} />
          <Button size="sm" disabled={!canWrite} title={canWrite ? undefined : "Importing a theme needs the admin role"} onClick={() => file.current?.click()}>
            Import JSON
          </Button>
        </div>
        <Table aria-label="Themes">
          <THead>
            <TR>
              <TH>Theme</TH>
              <TH>Colours</TH>
              <TH>Starts from</TH>
              <TH className="text-right">Actions</TH>
            </TR>
          </THead>
          <TBody>
            {(["light", "dark"] as const).map((b) => {
              const t = builtin(b);
              return (
                <TR key={b}>
                  <TD>{t.name} <span className="text-ink-muted">(built in)</span></TD>
                  <TD><Swatches theme={t} /></TD>
                  <TD className="text-ink-muted">–</TD>
                  <TD className="text-right">
                    {mark(t, b)}
                    <Button size="sm" variant="ghost" disabled={!canWrite} aria-label={`Duplicate ${t.name}`} onClick={() => setDraft({ theme: { name: "", base: b, tokens: { ...BUILTIN[b] } } })}>
                      Duplicate
                    </Button>
                  </TD>
                </TR>
              );
            })}
            {saved.map((t) => (
              <TR key={t.name}>
                <TD className="font-medium">{t.name}</TD>
                <TD><Swatches theme={t} /></TD>
                <TD className="text-ink-muted">{t.base === "light" ? "Light" : "Dark"}</TD>
                <TD className="text-right">
                  {mark(t, t.name)}
                  <Button size="sm" variant="ghost" aria-label={`Edit ${t.name}`} disabled={!canWrite} onClick={() => setDraft({ original: t.name, theme: t })}>
                    Edit
                  </Button>
                  <Button size="sm" variant="ghost" aria-label={`Export ${t.name}`} onClick={() => download(t)}>
                    Export
                  </Button>
                  <Button size="sm" variant="danger" aria-label={`Delete ${t.name}`} disabled={!canWrite} onClick={() => del(t)}>
                    Delete
                  </Button>
                </TD>
              </TR>
            ))}
          </TBody>
        </Table>
      </Section>

      {draft && (
        <ThemeEditor
          draft={draft.theme}
          onChange={(theme) => setDraft({ ...draft, theme })}
          onCancel={() => setDraft(null)}
          taken={saved.map((t) => t.name).filter((n) => n !== draft.original)}
          canWrite={canWrite}
          saving={save.isPending}
          onSave={() =>
            run(async () => {
              await save.mutateAsync(draft.theme);
              if (draft.original && draft.original !== draft.theme.name) await remove.mutateAsync(draft.original);
              setDraft(null);
            })
          }
        />
      )}
      {dialog}
    </div>
  );
}
