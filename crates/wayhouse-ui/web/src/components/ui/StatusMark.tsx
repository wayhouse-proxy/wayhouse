import { cn } from "../../lib/cn";

export type State = "up" | "degraded" | "down" | "unknown";

const WORD: Record<State, string> = { up: "Up", degraded: "Degraded", down: "Down", unknown: "Unknown" };

/** State is a shape, a colour and a word, always together (components.md, Status). */
export function StatusMark({ state, word, className }: { state: State; word?: React.ReactNode; className?: string }) {
  return (
    <span data-state={state} className={cn("inline-flex items-center gap-1.5 text-sm text-ink", className)}>
      {state === "up" && <span aria-hidden className="h-[9px] w-[9px] rounded-full bg-good" />}
      {state === "degraded" && (
        <span
          aria-hidden
          className="h-0 w-0 border-x-[5.5px] border-b-[10px] border-x-transparent border-b-warn"
        />
      )}
      {state === "down" && <span aria-hidden className="h-[9px] w-[9px] bg-bad" />}
      {state === "unknown" && <span aria-hidden className="h-[9px] w-[9px] rounded-full border-[1.5px] border-ink-muted" />}
      {word ?? WORD[state]}
    </span>
  );
}
