import { useState } from "react";
import { useSearchParams, Link } from "react-router-dom";
import { Plus, Trash2 } from "lucide-react";
import { ConfigSaveBar } from "../components/config/ConfigSaveBar";
import { SchemaForm } from "../components/form/SchemaForm";
import { AlertBar, PageHeader } from "../components/shell/Page";
import { EmptyState, ErrorState, Loading } from "../components/shell/States";
import { Button } from "../components/ui/Button";
import { useConfirm } from "../components/ui/ConfirmDialog";
import { Switch } from "../components/ui/Switch";
import { Table, TBody, TD, TH, THead, TR } from "../components/ui/Table";
import { useSharedConfigDoc } from "../lib/config/SharedConfigDoc";
import type { Json } from "../lib/editors/listeners";
import { usedBy } from "../lib/editors/lists";
import { get, set } from "../lib/form/path";
import { useFormLayout } from "../lib/form/useFormLayout";
import { useCan } from "../lib/session";

export interface ListPageProps {
  /** The top-level list in the configuration: `pools`, `backend_sources` or `resolvers`. */
  listKey: "pools" | "backend_sources" | "resolvers";
  noun: string;
  title: string;
  summary: string;
  /** The layout page the generated form shows. */
  page: string;
  /** `usedBy` kind, to warn before removing something that is referred to. */
  refKind: "pool" | "resolver" | "source";
  columns: { label: string; cell: (item: Json) => React.ReactNode; numeric?: boolean }[];
  create: (items: Json[]) => Json;
  /** Applied to the open item after every form edit: drops fields the item's current choices hide. */
  tidy?: (item: Json) => Json;
  /** The form fields of the open item; default all. */
  fieldsOf?: (item: Json) => string[] | undefined;
  /** Hand-edited parts above the form: the diagram, a type picker. */
  above?: (ctx: { item: Json; index: number; items: Json[]; root: Json; update: (item: Json) => void; canWrite: boolean }) => React.ReactNode;
}

/**
 * One top-level named list of the configuration as a page: a table of its items (the open one in the URL by position, since
 * the name is an editable field), Add, Remove with a note of where it is used, and the generated form for the open one.
 */
export function ConfigListPage(p: ListPageProps) {
  const doc = useSharedConfigDoc();
  const { layout, notice } = useFormLayout(doc.schema);
  const canWrite = useCan("admin");
  const { confirm, dialog } = useConfirm();
  const [params, setParams] = useSearchParams();
  const [advanced, setAdvanced] = useState(false);

  const root = (doc.value ?? {}) as Json;
  const items: Json[] = Array.isArray(root[p.listKey]) ? root[p.listKey] : [];
  const index = Math.min(Math.max(0, Number(params.get("i")) || 0), Math.max(0, items.length - 1));
  const current = items[index];

  if (doc.loading) return <Loading />;
  if (doc.error || !doc.schema) return <ErrorState onRetry={doc.reload}>{doc.error ?? "The configuration schema is not available."}</ErrorState>;

  const put = (path: (string | number)[], next: unknown) => doc.setValue(set(doc.value ?? {}, path, next));
  const select = (i: number) => setParams({ i: String(i) }, { replace: true });

  return (
    <div className="max-w-4xl">
      <PageHeader
        title={p.title}
        summary={p.summary}
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
          {items.length === 0 ? (
            <EmptyState>{`No ${p.noun} is configured yet.`}</EmptyState>
          ) : (
            <Table aria-label={p.title}>
              <THead>
                <TR>
                  <TH>Name</TH>
                  {p.columns.map((c) => (
                    <TH key={c.label} numeric={c.numeric}>
                      {c.label}
                    </TH>
                  ))}
                </TR>
              </THead>
              <TBody>
                {items.map((it, i) => (
                  <TR key={i} selected={i === index}>
                    <TD className="font-mono">
                      <Link replace className="underline" aria-current={i === index || undefined} to={`?i=${i}`}>
                        {String(it.name ?? "(unnamed)")}
                      </Link>
                    </TD>
                    {p.columns.map((c) => (
                      <TD key={c.label} numeric={c.numeric}>
                        {c.cell(it)}
                      </TD>
                    ))}
                  </TR>
                ))}
              </TBody>
            </Table>
          )}
          <div className="my-3 flex gap-2">
            <Button
              disabled={!canWrite}
              onClick={() => {
                put([p.listKey], [...items, p.create(items)]);
                select(items.length);
              }}
            >
              <Plus aria-hidden size={14} strokeWidth={1.5} /> Add {p.noun}
            </Button>
          </div>

          {current && (
            <section aria-label={`${p.noun} ${current.name}`} className="mt-4">
              <div className="mb-2 flex items-center justify-between gap-2">
                <h2 className="font-mono text-lg font-semibold">{String(current.name ?? "")}</h2>
                <Button
                  variant="danger"
                  disabled={!canWrite}
                  onClick={async () => {
                    const refs = usedBy(root, p.refKind, String(current.name));
                    const ok = await confirm({
                      title: `Remove ${p.noun} ${current.name}?`,
                      description: `${refs.length ? `It is used by ${refs.join(", ")}; they will point at nothing. ` : ""}Nothing changes until you apply.`,
                      confirmLabel: "Remove",
                    });
                    if (ok) put([p.listKey], items.filter((_, i) => i !== index));
                  }}
                >
                  <Trash2 aria-hidden size={14} strokeWidth={1.5} /> Remove {p.noun}
                </Button>
              </div>
              {p.above?.({ item: current, index, items, root, canWrite, update: (item) => put([p.listKey, index], item) })}
              <SchemaForm
                schema={doc.schema}
                value={doc.value}
                saved={doc.savedValue}
                onChange={(next) => {
                  const item = get(next, [p.listKey, index]);
                  const tidy = item && p.tidy ? p.tidy(item) : item;
                  doc.setValue(tidy === item ? next : set(next, [p.listKey, index], tidy));
                }}
                layout={layout}
                path={[p.listKey, index]}
                page={p.page}
                only={p.fieldsOf?.(current)}
                showAdvanced={advanced}
                readonly={!canWrite}
              />
            </section>
          )}
        </>
      )}
      <ConfigSaveBar doc={doc} />
      {dialog}
    </div>
  );
}
