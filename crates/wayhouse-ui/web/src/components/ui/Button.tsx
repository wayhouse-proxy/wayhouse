import { forwardRef } from "react";
import { cva, type VariantProps } from "class-variance-authority";
import { Loader2 } from "lucide-react";
import { cn } from "../../lib/cn";

export { Input } from "./Input";

const button = cva(
  `inline-flex items-center justify-center gap-1.5 rounded-sm font-semibold whitespace-nowrap
   transition-colors duration-(--dur-fast) ease-(--ease-out) aria-disabled:cursor-not-allowed aria-disabled:opacity-50`,
  {
    variants: {
      variant: {
        primary: "bg-primary text-primary-ink border border-primary hover:opacity-90 active:opacity-80",
        secondary:
          "bg-surface text-ink border border-control hover:bg-surface-sunken active:bg-line",
        danger: "bg-surface text-bad border border-control hover:bg-surface-sunken active:bg-line",
        ghost: "text-ink border border-transparent hover:bg-surface-sunken active:bg-line",
      },
      size: {
        default: "min-h-(--control-height) px-[11px] py-[5px] text-sm",
        sm: "min-h-6 px-2 py-0.5 text-xs",
      },
    },
    defaultVariants: { variant: "secondary", size: "default" },
  },
);

export type ButtonProps = React.ButtonHTMLAttributes<HTMLButtonElement> &
  VariantProps<typeof button> & { loading?: boolean };

/**
 * `disabled` and `loading` set aria-disabled and swallow clicks instead of using the
 * native attribute, so the button stays focusable and a tooltip or adjacent hint can say why.
 */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant, size, loading, disabled, className, onClick, children, type = "button", ...props },
  ref,
) {
  const inert = disabled || loading;
  return (
    <button
      ref={ref}
      type={type}
      aria-disabled={inert || undefined}
      aria-busy={loading || undefined}
      onClick={inert ? (e) => e.preventDefault() : onClick}
      className={cn(button({ variant, size }), className)}
      {...props}
    >
      {children}
      {loading && <Loader2 aria-hidden size={14} strokeWidth={1.5} className="animate-spin motion-reduce:animate-none" />}
    </button>
  );
});
