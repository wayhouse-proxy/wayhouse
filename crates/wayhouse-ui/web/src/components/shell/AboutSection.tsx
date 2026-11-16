import { safeNotesUrl, useRelease } from "../../lib/release";

/** Settings > About: the running version and, if the feed says so, the newest stable one. */
export function AboutSection() {
  const { data } = useRelease();
  if (!data) return null;
  const latest = data.latest;
  const notes = latest ? safeNotesUrl(latest.notes_url) : null;
  return (
    <section className="mt-8" aria-labelledby="about-heading">
      <h2 id="about-heading" className="mb-2 text-lg font-semibold">
        About
      </h2>
      <p className="text-sm">Running {data.current}.</p>
      {!data.enabled ? (
        <p className="text-sm text-ink-muted">The release check is off.</p>
      ) : latest ? (
        <p className="text-sm">
          Latest stable {latest.version} ({latest.date}).{" "}
          {notes && (
            <a href={notes} target="_blank" rel="noopener noreferrer" className="underline">
              Release notes
            </a>
          )}
        </p>
      ) : (
        <p className="text-sm text-ink-muted">No newer stable release is known.</p>
      )}
    </section>
  );
}
