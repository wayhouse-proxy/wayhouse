import * as RadixDialog from "@radix-ui/react-dialog";

export function Dialog({
  open,
  onOpenChange,
  title,
  description,
  wide,
  children,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: string;
  /** For content that needs the room, such as a diff. */
  wide?: boolean;
  children: React.ReactNode;
}) {
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      <RadixDialog.Portal>
        <RadixDialog.Overlay className="fixed inset-0 bg-ink/40" />
        <RadixDialog.Content
          className={`fixed top-1/2 left-1/2 max-h-[90vh] w-full ${wide ? "max-w-3xl" : "max-w-md"} -translate-x-1/2 -translate-y-1/2 rounded-lg
            border border-line bg-surface p-5 shadow-overlay focus:outline-none overflow-y-auto`}
        >
          <RadixDialog.Title className="mb-3 text-lg font-semibold text-ink">{title}</RadixDialog.Title>
          {description ? (
            <RadixDialog.Description className="mb-4 text-sm text-ink-muted">{description}</RadixDialog.Description>
          ) : (
            <RadixDialog.Description className="sr-only">{title}</RadixDialog.Description>
          )}
          {children}
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
