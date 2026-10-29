import { lazy, Suspense } from "react";
import { ErrorState, Loading } from "../components/shell/States";
import { AlertBar, PageHeader } from "../components/shell/Page";
import { ConfigSaveBar } from "../components/config/ConfigSaveBar";
import { useSharedConfigDoc } from "../lib/config/SharedConfigDoc";
import { useCan } from "../lib/session";

// The editor is the second heaviest dependency after the charts, so it loads on first visit.
const SourceEditor = lazy(() => import("../components/config/SourceEditor"));

/** Settings > Source: the whole configuration as YAML. Every field is reachable here, comments included. */
export function SourcePage() {
  const doc = useSharedConfigDoc();
  const canWrite = useCan("admin");
  if (doc.loading) return <Loading />;
  if (doc.error) return <ErrorState onRetry={doc.reload}>{doc.error}</ErrorState>;
  return (
    <div className="max-w-4xl">
      <PageHeader
        title="Source"
        summary={doc.revision === null ? "No revision submitted yet" : `Revision ${doc.revision}. The same file the forms edit; comments and unknown keys are kept.`}
      />
      {doc.parseError && <AlertBar>The YAML does not parse, so the forms are off until it does: {doc.parseError}</AlertBar>}
      <Suspense fallback={<Loading />}>
        <SourceEditor value={doc.text} onChange={doc.setText} readOnly={!canWrite} label="Configuration YAML" />
      </Suspense>
      <ConfigSaveBar doc={doc} />
    </div>
  );
}
