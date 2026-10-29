import { SchemaForm } from "../components/form/SchemaForm";
import { ConfigSaveBar } from "../components/config/ConfigSaveBar";
import { AlertBar, PageHeader } from "../components/shell/Page";
import { ErrorState, Loading } from "../components/shell/States";
import { Switch } from "../components/ui/Switch";
import { useSharedConfigDoc } from "../lib/config/SharedConfigDoc";
import { useFormLayout } from "../lib/form/useFormLayout";
import { useState } from "react";
import { useCan } from "../lib/session";

/** Settings > General: the `settings:` part of the configuration as forms, from the schema and the layout. */
export function SettingsPage() {
  const doc = useSharedConfigDoc();
  const { layout, notice } = useFormLayout(doc.schema);
  const [advanced, setAdvanced] = useState(false);
  const canWrite = useCan("admin");
  if (doc.loading) return <Loading />;
  if (doc.error || !doc.schema) return <ErrorState onRetry={doc.reload}>{doc.error ?? "The configuration schema is not available."}</ErrorState>;
  return (
    <div className="max-w-3xl">
      <PageHeader
        title="Settings"
        summary={doc.revision === null ? "No revision submitted yet" : `Revision ${doc.revision}`}
        actions={
          <label className="flex items-center gap-2 text-sm text-ink-muted">
            <Switch checked={advanced} onCheckedChange={setAdvanced} aria-label="Show advanced" />
            Show advanced
          </label>
        }
      />
      {notice && <AlertBar>{notice}</AlertBar>}
      {doc.parseError ? (
        <AlertBar>The YAML on the Source page does not parse, so the forms are off until it does: {doc.parseError}</AlertBar>
      ) : (
        <SchemaForm schema={doc.schema} value={doc.value} saved={doc.savedValue} onChange={doc.setValue} layout={layout} page="global" showAdvanced={advanced} readonly={!canWrite} />
      )}
      <ConfigSaveBar doc={doc} />
    </div>
  );
}
