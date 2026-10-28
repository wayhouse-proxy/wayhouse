import * as Collapsible from "@radix-ui/react-collapsible";
import { ChevronRight } from "lucide-react";
import { cn } from "../../lib/cn";

/**
 * A collapsible group of related fields. Open by default unless `advanced`; the
 * summary shows "N changed" when something inside differs from the saved value.
 */
export function Section({
  title,
  changed = 0,
  advanced,
  defaultOpen,
  children,
  className,
}: {
  title: string;
  changed?: number;
  advanced?: boolean;
  defaultOpen?: boolean;
  children: React.ReactNode;
  className?: string;
}) {
  return (
    <Collapsible.Root defaultOpen={defaultOpen ?? !advanced} className={cn("border-b border-line", className)}>
      <Collapsible.Trigger
        className="group flex min-h-(--control-height) w-full items-center gap-1.5 text-left text-sm font-semibold text-ink"
      >
        <ChevronRight
          aria-hidden
          size={16}
          strokeWidth={1.5}
          className="transition-transform duration-(--dur-fast) group-data-[state=open]:rotate-90"
        />
        {title}
        {changed > 0 && <span className="ml-2 text-xs font-normal text-ink-muted">{changed} changed</span>}
      </Collapsible.Trigger>
      <Collapsible.Content className="pb-2 pl-5">{children}</Collapsible.Content>
    </Collapsible.Root>
  );
}
