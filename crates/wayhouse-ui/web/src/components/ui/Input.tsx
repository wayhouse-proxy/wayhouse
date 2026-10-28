import { forwardRef } from "react";
import { ChevronDown } from "lucide-react";
import { cn } from "../../lib/cn";

const control = `min-h-(--control-height) rounded-sm border border-control bg-surface px-2 py-[5px] text-sm text-ink
  placeholder:text-ink-muted transition-colors duration-(--dur-fast) hover:border-ink-muted
  aria-disabled:cursor-not-allowed aria-disabled:opacity-50 disabled:cursor-not-allowed disabled:opacity-50
  aria-invalid:border-bad`;

export const Input = forwardRef<HTMLInputElement, React.InputHTMLAttributes<HTMLInputElement> & { mono?: boolean }>(
  function Input({ className, mono, ...props }, ref) {
    return <input ref={ref} className={cn(control, mono && "font-mono", className)} {...props} />;
  },
);

/** A number with its unit as muted text on the right ("seconds", "per second"). */
export const NumberInput = forwardRef<
  HTMLInputElement,
  Omit<React.InputHTMLAttributes<HTMLInputElement>, "type"> & { unit?: string }
>(function NumberInput({ unit, className, ...props }, ref) {
  return (
    <span className="inline-flex items-center gap-2">
      <input
        ref={ref}
        type="number"
        className={cn(control, "w-[110px] text-right tabular-nums", className)}
        {...props}
      />
      {unit && <span className="text-xs text-ink-muted">{unit}</span>}
    </span>
  );
});

/** Lists such as CIDRs, one per line. */
export const Textarea = forwardRef<HTMLTextAreaElement, React.TextareaHTMLAttributes<HTMLTextAreaElement>>(
  function Textarea({ className, ...props }, ref) {
    return <textarea ref={ref} className={cn(control, "font-mono", className)} {...props} />;
  },
);

/** The native select, styled to match; Radix Select only when options need search or groups. */
export const Select = forwardRef<HTMLSelectElement, React.SelectHTMLAttributes<HTMLSelectElement>>(
  function Select({ className, children, ...props }, ref) {
    return (
      <span className="relative inline-flex items-center">
        <select ref={ref} className={cn(control, "appearance-none pr-7", className)} {...props}>
          {children}
        </select>
        <ChevronDown aria-hidden size={16} strokeWidth={1.5} className="pointer-events-none absolute right-2 text-ink-muted" />
      </span>
    );
  },
);
