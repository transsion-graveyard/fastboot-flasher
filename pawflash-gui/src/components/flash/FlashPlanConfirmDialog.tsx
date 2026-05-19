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
    {
      key: "review",
      title: "Review flash plan",
      detail: plan ? `${plan.mode} on ${plan.storage} storage with ${plan.slot_policy} slot policy.` : "No plan loaded yet.",
      icon: Sparkles,
      tone: "text-accent-brand",
    },
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
      <DialogContent className="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>Confirm flash plan</DialogTitle>
          <DialogDescription className="max-w-[60ch] leading-6">
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

          <ScrollArea className="max-h-[46vh] pr-2">
            <div className="space-y-3">
              {steps.map((step, index) => {
                const Icon = step.icon;
                return (
                  <div key={step.key} className="flex gap-3">
                    <div className="flex w-8 shrink-0 flex-col items-center">
                      <div className={cn("flex h-8 w-8 items-center justify-center rounded-full border bg-background", step.tone)}>
                        <Icon className="h-4 w-4" />
                      </div>
                      {index < steps.length - 1 && <div className="mt-2 h-full min-h-8 w-px flex-1 bg-border" />}
                    </div>
                    <div className="min-w-0 flex-1 rounded-md border border-border bg-muted/20 px-3 py-3">
                      <p className="text-sm font-semibold">{step.title}</p>
                      <p className="mt-1 text-sm leading-6 text-muted-foreground">{step.detail}</p>
                      {"items" in step && step.items && step.items.length > 0 ? (
                        <ul className="mt-3 space-y-2">
                          {step.items.map((item) => (
                            <li key={`${step.key}-${item.label}`} className="rounded-sm border border-border/70 bg-background px-3 py-2 text-sm">
                              <div className="flex items-start justify-between gap-3">
                                <span className="min-w-0 truncate font-medium">{item.label}</span>
                                <span className="shrink-0 text-xs uppercase tracking-[0.12em] text-muted-foreground">{item.kind}</span>
                              </div>
                              {item.detail ? (
                                <p className="mt-1 truncate text-xs text-muted-foreground">{item.detail}</p>
                              ) : null}
                            </li>
                          ))}
                        </ul>
                      ) : null}
                    </div>
                  </div>
                );
              })}
            </div>
          </ScrollArea>
        </div>

        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={isPending}>
            Cancel
          </Button>
          <Button onClick={onConfirm} disabled={isPending}>
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
