import { useState } from "react";
import { ArrowUpCircle, X } from "lucide-react";
import { dismissedVersion, dismissVersion, safeNotesUrl, useRelease } from "../../lib/release";

/** A small, dismissible note in the shell when a newer stable release exists. It never blocks and never installs. */
export function ReleaseBadge() {
  const { data } = useRelease();
  const [gone, setGone] = useState(dismissedVersion());
  const latest = data?.latest;
  if (!latest || gone === latest.version) return null;
  const notes = safeNotesUrl(latest.notes_url);
  return (
    <div className="mb-3 flex flex-wrap items-center gap-2 border border-line bg-surface px-3 py-1.5 text-sm" role="status">
      <ArrowUpCircle aria-hidden size={16} strokeWidth={1.5} className="shrink-0 text-ink-muted" />
      <span>wayhouse {latest.version} is out.</span>
      {notes && (
        <a href={notes} target="_blank" rel="noopener noreferrer" className="underline">
          Release notes
        </a>
      )}
      <button
        type="button"
        aria-label={`Dismiss the note about ${latest.version}`}
        className="ml-auto text-ink-muted hover:text-ink"
        onClick={() => {
          dismissVersion(latest.version);
          setGone(latest.version);
        }}
      >
        <X aria-hidden size={14} strokeWidth={1.5} />
      </button>
    </div>
  );
}
