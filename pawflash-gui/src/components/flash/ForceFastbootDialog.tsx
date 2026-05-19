import { Dialog as DialogPrimitive } from "@base-ui/react/dialog";
import { CheckCircle2, LoaderCircle, Minus, X, XCircle } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useForceFastboot } from "@/hooks/useForceFastboot";
import {
  createDismissibleDialogRootHandler,
  type DialogChangeReason,
} from "@/components/shared/dialogBehavior";

interface ForceFastbootDialogProps {
  open: boolean;
  onOpenChange: (open: boolean, reason?: DialogChangeReason) => void;
  onHide: () => void;
  onCancel: () => void | Promise<void>;
}

export function ForceFastbootDialog({
  open,
  onOpenChange,
  onHide,
  onCancel,
}: ForceFastbootDialogProps) {
  const { phase, message } = useForceFastboot();
  const canMinimize = phase === "waiting";
  const isFinished = phase === "complete" || phase === "cancelled" || phase === "error";

  return (
    <DialogPrimitive.Root
      open={open}
      onOpenChange={createDismissibleDialogRootHandler(onOpenChange)}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Backdrop className="fixed inset-0 z-50 bg-stone-950/18 backdrop-blur-sm transition-opacity duration-150 data-closed:opacity-0 data-open:opacity-100" />
        <DialogPrimitive.Popup className="fixed top-1/2 left-1/2 z-50 flex w-[min(28rem,calc(100vw-1rem))] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-md border border-border bg-background shadow-[var(--overlay-shadow)] pointer-events-auto outline-none transition-all duration-150 data-closed:scale-[0.99] data-closed:opacity-0 data-open:scale-100 data-open:opacity-100">
          <div className="flex flex-wrap items-center justify-between gap-3 border-b border-border px-4 py-3">
            <div className="min-w-0 flex-1">
              <DialogPrimitive.Title className="min-w-0 flex-1 text-base font-semibold">
                {phase === "error"
                  ? "Force fastboot failed"
                  : phase === "complete"
                    ? "Force fastboot complete"
                    : phase === "cancelled"
                      ? "Force fastboot cancelled"
                      : "Waiting for preloader..."}
              </DialogPrimitive.Title>
            </div>
            <div className="relative z-10 flex shrink-0 items-center gap-2">
              {phase === "waiting" && (
                <Button variant="outline" size="sm" className="rounded-sm whitespace-nowrap" onClick={onCancel}>
                  <X className="h-3.5 w-3.5" />
                  Cancel
                </Button>
              )}
              {canMinimize ? (
                <Button variant="outline" size="sm" className="rounded-sm whitespace-nowrap" onClick={onHide}>
                  <Minus className="h-4 w-4" />
                  Minimize
                </Button>
              ) : isFinished ? (
                <Button variant="outline" size="sm" className="rounded-sm whitespace-nowrap" onClick={() => onOpenChange(false)}>
                  <X className="h-4 w-4" />
                  Close
                </Button>
              ) : null}
            </div>
          </div>

          <div className="space-y-3 px-4 py-4">
            <div className="status-shell flex items-center gap-3 px-3 py-3">
              {phase === "complete" ? (
                <CheckCircle2 className="h-5 w-5 shrink-0 text-success" />
              ) : phase === "cancelled" || phase === "error" ? (
                <XCircle className={cn("h-5 w-5 shrink-0", phase === "cancelled" ? "text-warning" : "text-error")} />
              ) : (
                <LoaderCircle className="h-5 w-5 shrink-0 animate-spin text-accent-brand" />
              )}
              <div className="min-w-0">
                <p className="text-sm font-medium">
                  {phase === "complete"
                    ? "Device is forced to reboot into FASTBOOT mode"
                    : phase === "cancelled"
                      ? "Force fastboot cancelled"
                      : phase === "error"
                      ? "Operation stopped"
                      : "Listening for preloader handshake"}
                </p>
                {message && (
                  <p
                    className={cn(
                      "mt-1 break-words text-sm",
                      phase === "cancelled"
                        ? "text-warning"
                        : phase === "error"
                          ? "text-error"
                          : "text-muted-foreground",
                    )}
                  >
                    {message}
                  </p>
                )}
              </div>
            </div>
          </div>
        </DialogPrimitive.Popup>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
