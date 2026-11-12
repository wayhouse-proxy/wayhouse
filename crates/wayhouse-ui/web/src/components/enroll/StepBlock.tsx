import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { Button } from "../ui/Button";
import type { Step } from "../../lib/enroll/commands";

/** One generated step: a title, the text in a code block, a Copy button and its note. */
export function StepBlock({ step, index }: { step: Step; index: number }) {
  const [copied, setCopied] = useState<"yes" | "failed" | null>(null);

  async function copy() {
    try {
      await navigator.clipboard.writeText(step.text);
      setCopied("yes");
    } catch {
      // No clipboard permission (plain http, an embedded view): the text is selectable below.
      setCopied("failed");
    }
  }

  return (
    <section aria-label={`Step ${index + 1}: ${step.title}`} className="mb-4">
      <div className="mb-1 flex flex-wrap items-center justify-between gap-2">
        <h3 className="text-sm font-semibold text-ink">
          {index + 1}. {step.title}
        </h3>
        <Button size="sm" onClick={copy} aria-label={`Copy step ${index + 1}`}>
          {copied === "yes" ? <Check aria-hidden size={14} strokeWidth={1.5} /> : <Copy aria-hidden size={14} strokeWidth={1.5} />}
          {copied === "yes" ? "Copied" : "Copy"}
        </Button>
      </div>
      <pre
        tabIndex={0}
        aria-label={`Step ${index + 1} text`}
        className="overflow-x-auto border border-line bg-surface-sunken p-3 font-mono text-xs leading-relaxed text-ink"
      >
        {step.text}
      </pre>
      {copied === "failed" && (
        <p role="status" className="mt-1 text-xs text-bad">
          The browser refused to copy. Select the text above and copy it by hand.
        </p>
      )}
      {step.holdsCode && (
        <p className="mt-1 max-w-(--measure) text-xs text-ink-muted">
          Copying puts the code on the clipboard, where a clipboard manager may keep it. Clear it once pasted.
        </p>
      )}
      {step.note && <p className="mt-1 max-w-(--measure) text-xs text-ink-muted">{step.note}</p>}
    </section>
  );
}
