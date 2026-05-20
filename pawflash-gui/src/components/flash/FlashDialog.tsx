import { useEffect } from "react";
import { Dialog as DialogPrimitive } from "@base-ui/react/dialog";
import {
  Minus,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { cn } from "@/lib/utils";
import { useFlashProgress } from "@/hooks/useFlashProgress";
import {
  createDismissibleDialogRootHandler,
  type DialogChangeReason,
} from "@/components/shared/dialogBehavior";

interface FlashDialogProps {
  open: boolean;
  onOpenChange: (open: boolean, reason?: DialogChangeReason) => void;
  onMinimize: () => void;
  onCancel: () => void | Promise<void>;
  canCancel: boolean;
}

export function FlashDialog({
  open,
  onOpenChange,
  onMinimize,
  onCancel,
  canCancel,
}: FlashDialogProps) {
  const {
    phase,
    operation,
    partition,
    bytes,
    total,
    speedBps,
    overallBytes,
    overallTotal,
    summary,
    errorMessage,
    statusText,
    setIsMinimized,
  } = useFlashProgress();

  useEffect(() => {
    if (open) setIsMinimized(false);
  }, [open, setIsMinimized]);

  const imagePct = total > 0 ? Math.round((bytes / total) * 100) : 0;
  const overallPct = overallTotal > 0 ? Math.round((overallBytes / overallTotal) * 100) : 0;
  const tone = phaseTone(phase);
  const isFinished = phase === "complete" || phase === "cancelled" || phase === "error";
  const showCurrentCard = phase !== "complete";

  return (
    <DialogPrimitive.Root
      open={open}
      onOpenChange={createDismissibleDialogRootHandler(onOpenChange)}
    >
      <DialogPrimitive.Portal>
        <DialogPrimitive.Backdrop className="fixed inset-0 z-50 bg-stone-950/18 backdrop-blur-sm transition-opacity duration-150 data-closed:opacity-0 data-open:opacity-100" />
        <DialogPrimitive.Popup
          data-slot="flash-dialog"
          className={cn(
            "fixed top-1/2 left-1/2 z-50 flex w-[min(48rem,calc(100vw-1rem))] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-md border border-border bg-background shadow-[var(--overlay-shadow)] pointer-events-auto outline-none transition-all duration-150 data-closed:scale-[0.99] data-closed:opacity-0 data-open:scale-100 data-open:opacity-100",
            !isFinished && "min-h-[15rem]",
          )}
        >
          <div className="grid gap-3 border-b border-border px-4 py-3 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
            <div className="min-w-0">
              <DialogPrimitive.Title className={cn("truncate text-base font-semibold", tone.title)}>
                {compactTitle(phase, operation)}
              </DialogPrimitive.Title>
            </div>
            <div className="relative z-10 flex shrink-0 flex-wrap items-center justify-end gap-2">
              {canCancel && (
                <Button
                  variant="outline"
                  size="sm"
                  className="w-full rounded-sm whitespace-nowrap sm:w-auto"
                  onClick={onCancel}
                >
                  <X className="h-3.5 w-3.5" />
                  Cancel
                </Button>
              )}
              {canCancel ? (
                <Button
                  variant="outline"
                  size="sm"
                  className="w-full rounded-sm whitespace-nowrap sm:w-auto"
                  aria-label="Hide flash dialog"
                  onClick={() => {
                    setIsMinimized(true);
                    onMinimize();
                  }}
                >
                  <Minus className="h-4 w-4" />
                  Minimize
                </Button>
              ) : isFinished ? (
                <Button
                  variant="outline"
                  size="sm"
                  className="w-full rounded-sm whitespace-nowrap sm:w-auto"
                  onClick={() => onOpenChange(false)}
                >
                  <X className="h-4 w-4" />
                  Close
                </Button>
              ) : null}
            </div>
          </div>

          <div className="space-y-4 px-4 py-4">
            {errorMessage && (phase === "error" || phase === "cancelled") && (
              <p className={cn("rounded-sm border px-3 py-2 text-sm break-words leading-6", phase === "cancelled" ? "border-warning/20 bg-warning/8 text-warning" : "border-error/20 bg-error/8 text-error")}>
                {errorMessage}
              </p>
            )}

            {statusText && !errorMessage && (
              <p className="text-center text-xs font-medium uppercase tracking-[0.12em] text-muted-foreground">
                {statusText}
              </p>
            )}

            <div className={cn("grid gap-3", showCurrentCard ? "xl:grid-cols-[minmax(0,1.12fr)_minmax(0,0.88fr)]" : "grid-cols-1")}>
              {showCurrentCard && (
                <section className="status-shell grid gap-4 px-4 py-4">
                  <div className="min-w-0">
                    <ProgressBlock
                      label={currentProgressLabel(phase, operation)}
                      value={phase === "waiting" ? 0 : imagePct}
                      toneClass={tone.bar}
                      caption={currentProgressCaption(phase, operation, partition, statusText)}
                      amount={phase === "waiting" ? "" : formatBytesProgress(bytes, total)}
                    />
                  </div>

                  <Metric
                    className="w-fit justify-self-end text-right"
                    label="Transfer speed"
                    value={phase === "flashing" && speedBps > 0 ? formatSpeed(speedBps) : "—"}
                  />
                </section>
              )}

              <section className="status-shell space-y-4 px-4 py-4">
                <div className="flex items-start justify-between gap-3">
                  <div className="min-w-0">
                    <p className="mt-1 text-sm font-semibold leading-5">
                      {phase === "complete" ? "Finished" : "Cumulative transfer"}
                    </p>
                  </div>
                  <span className="shrink-0 text-sm font-semibold tabular-nums text-muted-foreground">
                    {overallPct}%
                  </span>
                </div>

                <ProgressBlock
                  label="Overall progress"
                  value={overallPct}
                  toneClass={tone.bar}
                  caption={overallCaption(phase, overallBytes, overallTotal)}
                  amount={overallTotal > 0 ? formatBytesProgress(overallBytes, overallTotal) : ""}
                />
              </section>
            </div>

            {summary && (
              <div className="grid grid-cols-2 gap-2 border-t border-border pt-3">
                <Metric label="Flashed" value={summary.flash_count} />
                <Metric label="Wiped" value={summary.wipe_count} />
                <Metric label="Skipped" value={summary.skipped_count} />
                <Metric label="Total" value={`${(summary.total_bytes / 1e9).toFixed(2)} GiB`} />
              </div>
            )}
          </div>
        </DialogPrimitive.Popup>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}

function ProgressBlock({
  label,
  value,
  toneClass,
  caption,
  amount,
}: {
  label: string;
  value: number;
  toneClass: string;
  caption: string;
  amount: string;
}) {
  return (
    <div className="space-y-1.5">
      <div className="flex items-center justify-between gap-3 text-xs font-medium uppercase tracking-[0.12em] text-muted-foreground">
        <span className="min-w-0 truncate">{label}</span>
        <span className="tabular-nums">{value}%</span>
      </div>
      <Progress value={value} indicatorClassName={toneClass} className="gap-0" />
      {(caption || amount) && (
        <div className="flex min-w-0 items-center justify-between gap-3 text-sm">
          <span className="min-w-0 flex-1 truncate text-muted-foreground">{caption}</span>
          <span className="shrink-0 tabular-nums text-muted-foreground">{amount}</span>
        </div>
      )}
    </div>
  );
}

function Metric({
  label,
  value,
  className,
}: {
  label: string;
  value: number | string;
  className?: string;
}) {
  return (
    <div className={cn("status-shell px-3 py-2", className)}>
      <p className="text-xs uppercase tracking-[0.12em] text-muted-foreground">{label}</p>
      <p className="mt-1 text-lg font-semibold tabular-nums">{value}</p>
    </div>
  );
}

function phaseTone(phase: "idle" | "waiting" | "flashing" | "complete" | "cancelled" | "error") {
  switch (phase) {
    case "error":
      return {
        title: "text-error",
        bar: "bg-error",
      };
    case "cancelled":
      return {
        title: "text-warning",
        bar: "bg-warning",
      };
    case "complete":
      return {
        title: "text-success",
        bar: "bg-success",
      };
    case "waiting":
      return {
        title: "text-foreground",
        bar: "bg-info",
      };
    default:
      return {
        title: "text-foreground",
        bar: "bg-accent-brand",
      };
  }
}

function compactTitle(
  phase: "idle" | "waiting" | "flashing" | "complete" | "cancelled" | "error",
  operation: "" | "flash" | "erase",
) {
  if (phase === "waiting") return "Waiting for device...";
  if (phase === "flashing") {
    return operation === "erase" ? "Erase progress" : "Flash progress";
  }
  if (phase === "complete") return "Flash complete";
  if (phase === "cancelled") return "Cancelled";
  if (phase === "error") return "Flash failed";
  return "Preparing...";
}

function currentProgressLabel(
  phase: "idle" | "waiting" | "flashing" | "complete" | "cancelled" | "error",
  operation: "" | "flash" | "erase",
) {
  if (phase === "waiting") return "Current step";
  return operation === "erase" ? "Current erase" : "Current partition";
}

function currentProgressCaption(
  phase: "idle" | "waiting" | "flashing" | "complete" | "cancelled" | "error",
  operation: "" | "flash" | "erase",
  partition: string,
  statusText = "",
) {
  if (phase === "waiting") return statusText || "No device connected";
  if (partition) return partition;
  return operation === "erase" ? "Preparing erase" : "Preparing partition";
}

function overallCaption(
  phase: "idle" | "waiting" | "flashing" | "complete" | "cancelled" | "error",
  overallBytes: number,
  overallTotal: number,
) {
  if (phase === "waiting") return "Waiting for device";
  if (phase === "complete") return "";
  if (phase === "cancelled") return "Stopped before finishing all actions";
  if (phase === "error") return "Stopped due to an error";
  if (overallTotal <= 0 && overallBytes <= 0) return "Preparing progress";
  return "Cumulative transfer";
}

function formatBytesProgress(bytes: number, total: number) {
  if (total <= 0) return "";
  return `${formatBytes(bytes)} / ${formatBytes(total)}`;
}

function formatBytes(value: number) {
  if (value <= 0) return "0 B";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let size = value;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }

  const precision = unitIndex === 0 ? 0 : size >= 100 ? 0 : size >= 10 ? 1 : 2;
  return `${size.toFixed(precision)} ${units[unitIndex]}`;
}

function formatSpeed(speedBps: number): string {
  if (speedBps <= 0) return "";
  const units = ["B/s", "KiB/s", "MiB/s", "GiB/s", "TiB/s"];
  let value = speedBps;
  let unitIndex = 0;

  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }

  const precision = unitIndex === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(precision)} ${units[unitIndex]}`;
}
