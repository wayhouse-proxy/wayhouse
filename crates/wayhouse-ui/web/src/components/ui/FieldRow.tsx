import { useId } from "react";
import { cn } from "../../lib/cn";

/**
 * Label and help on the left (240 px), control on the right; stacks under 760 px.
 * `render` receives the ids to put on the control so label, help and error are tied to it.
 */
export function FieldRow({
  label,
  help,
  yamlKey,
  changed,
  error,
  render,
  className,
  id: given,
}: {
  label: string;
  help?: string;
  yamlKey?: string;
  changed?: boolean;
  error?: string;
  render: (a: { id: string; "aria-describedby"?: string; "aria-invalid"?: true }) => React.ReactNode;
  className?: string;
  /** Use this id for the control instead of a generated one (a renderer that creates the control itself). */
  id?: string;
}) {
  const generated = useId();
  const id = given ?? generated;
  const described = [help && `${id}-help`, error && `${id}-error`].filter(Boolean).join(" ") || undefined;
  return (
    <div className={cn("grid gap-2 py-2 min-[760px]:grid-cols-[240px_1fr] min-[760px]:gap-4", className)}>
      <div>
        <label htmlFor={id} className="font-medium text-sm text-ink">
          {label}
          {changed && (
            <span
              aria-hidden
              title="Changed from the saved value"
              className="ml-1.5 inline-block h-1.5 w-1.5 rounded-full bg-select align-middle"
            />
          )}
          {changed && <span className="sr-only"> (changed)</span>}
        </label>
        {yamlKey && <code className="ml-1.5 font-mono text-xs text-ink-muted">{yamlKey}</code>}
        {help && (
          <p id={`${id}-help`} className="mt-0.5 max-w-(--measure) text-xs text-ink-muted">
            {help}
          </p>
        )}
      </div>
      <div>
        {render({ id, "aria-describedby": described, ...(error ? { "aria-invalid": true as const } : {}) })}
        {error && (
          <p id={`${id}-error`} className="mt-1 flex items-center gap-1.5 text-xs text-bad">
            <span aria-hidden className="inline-block h-[9px] w-[9px] bg-bad" />
            {error}
          </p>
        )}
      </div>
    </div>
  );
}
