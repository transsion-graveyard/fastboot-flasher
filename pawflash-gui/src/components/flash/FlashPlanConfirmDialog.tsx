import { memo } from "react";
import { Check, Eraser, RotateCcw, Sparkles, Zap } from "lucide-react";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import type { FlashPlanDto, PartitionDto } from "@/types/api";

interface FlashPlanConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void | Promise<void>;
  plan: FlashPlanDto | null;
  selectedPartitions: PartitionDto[];
  rebootAfter: boolean;
  isPending?: boolean;
}

export const FlashPlanConfirmDialog = memo(function FlashPlanConfirmDialog({
  open,
  onOpenChange,
  onConfirm,
  plan,
  selectedPartitions,
  rebootAfter,
  isPending = false,
}: FlashPlanConfirmDialogProps) {
  const flashPartitions = selectedPartitions.filter((partition) => partition.action === "flash");
  const wipePartitions = selectedPartitions.filter((partition) => partition.action === "wipe");
  const steps = [
    ...(flashPartitions.length > 0
      ? [{
          key: "flash",
          title: "Flash selected partitions",
          detail: `${flashPartitions.length} partition${flashPartitions.length === 1 ? "" : "s"} will be written.`,
          icon: Zap,
          tone: "text-success",
          items: flashPartitions.map((partition) => partitionLine(partition)),
        }]
      : []),
    ...(wipePartitions.length > 0
      ? [{
          key: "wipe",
          title: "Wipe selected partitions",
          detail: `${wipePartitions.length} partition${wipePartitions.length === 1 ? "" : "s"} will be erased.`,
          icon: Eraser,
          tone: "text-warning",
          items: wipePartitions.map((partition) => partitionLine(partition)),
        }]
      : []),
    ...(rebootAfter
      ? [{
          key: "reboot",
          title: "Reboot to system",
          detail: "Send reboot after flashing finishes.",
          icon: RotateCcw,
          tone: "text-info",
        }]
      : []),
    {
      key: "complete",
      title: "Complete",
      detail: "The dialog closes when flashing finishes successfully.",
      icon: Check,
      tone: "text-success",
    },
  ] as const;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(72rem,calc(100vw-1rem))] max-w-none gap-4">
        <DialogHeader>
          <DialogTitle>Confirm flash plan</DialogTitle>
          <DialogDescription className="max-w-[72ch] leading-6">
            Review the exact steps before the device starts flashing.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4">
          <div className="flex flex-wrap gap-2">
            {plan && (
              <>
                <Badge variant="outline">{plan.mode}</Badge>
                <Badge variant="outline">{plan.storage}</Badge>
                <Badge variant="outline">{plan.slot_policy}</Badge>
              </>
            )}
            <Badge variant="outline">{flashPartitions.length} flash</Badge>
            <Badge variant="outline">{wipePartitions.length} wipe</Badge>
            {rebootAfter && <Badge variant="outline">reboot after flash</Badge>}
          </div>

          <ScrollArea className="max-h-[50vh] pr-2">
            <div className="space-y-4">
              {plan && (
                <div className="grid gap-3 rounded-lg border border-border/60 bg-muted/20 p-3 sm:grid-cols-3 xl:grid-cols-4">
                  <InfoChip label="Mode" value={plan.mode} />
                  <InfoChip label="Storage" value={plan.storage} />
                  <InfoChip label="Slot" value={plan.slot_policy} />
                  <InfoChip label="Selected" value={`${selectedPartitions.length} partitions`} />
                </div>
              )}

              <div className="relative">
                <div className="pointer-events-none absolute left-8 right-8 top-7 hidden xl:block h-px bg-border/80" />
                <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-4">
                  {steps.map((step, index) => (
                    <StepCard key={step.key} step={step} index={index} total={steps.length} />
                  ))}
                </div>
              </div>
            </div>
          </ScrollArea>
        </div>

        <DialogFooter className="items-stretch sm:items-center">
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={isPending} className="w-full sm:w-auto">
            Cancel
          </Button>
          <Button onClick={onConfirm} disabled={isPending} className="w-full sm:w-auto">
            {isPending ? "Starting..." : "Start flash"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
});

function partitionLine(partition: PartitionDto) {
  return {
    label: partition.partition,
    detail: partition.action === "flash"
      ? partition.image_name ?? partition.image_path ?? "No image resolved"
      : partition.action,
    kind: partition.action,
  };
}

function InfoChip({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md border border-border/70 bg-background px-3 py-2">
      <p className="text-[10px] font-medium uppercase tracking-[0.16em] text-muted-foreground">{label}</p>
      <p className="mt-1 text-sm font-semibold leading-5">{value}</p>
    </div>
  );
}

function StepCard({
  step,
  index,
  total,
}: {
  step: {
    key: string;
    title: string;
    detail: string;
    icon: typeof Sparkles;
    tone: string;
    items?: Array<{
      label: string;
      detail: string;
      kind: string;
    }>;
  };
  index: number;
  total: number;
}) {
  const Icon = step.icon;
  const hasItems = Boolean(step.items && step.items.length > 0);

  return (
    <div className="relative overflow-hidden rounded-lg border border-border/70 bg-background px-3 py-3 shadow-sm">
      {index < total - 1 && <div className="pointer-events-none absolute right-0 top-7 hidden xl:block h-px w-6 translate-x-full bg-border/80" />}
      <div className="flex items-start gap-3">
        <div className={cn("flex h-9 w-9 shrink-0 items-center justify-center rounded-full border border-border/70 bg-card", step.tone)}>
          <Icon className="h-4 w-4" />
        </div>
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold leading-5">{step.title}</p>
          <p className="mt-1 text-sm leading-6 text-muted-foreground">{step.detail}</p>
        </div>
      </div>

      {hasItems ? (
        <div className="mt-3 grid gap-2">
          {step.items?.map((item) => (
            <div key={`${step.key}-${item.label}`} className="rounded-md border border-border/70 bg-muted/25 px-3 py-2">
              <div className="flex items-start justify-between gap-3">
                <span className="min-w-0 truncate text-sm font-medium">{item.label}</span>
                <span className="shrink-0 text-[10px] uppercase tracking-[0.16em] text-muted-foreground">{item.kind}</span>
              </div>
              <p className="mt-1 truncate text-xs text-muted-foreground">{item.detail}</p>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}
