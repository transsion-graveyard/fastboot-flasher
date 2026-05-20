import { memo, useMemo } from "react";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { flashModeLabel } from "@/lib/flash-mode";
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
  isPending = false,
}: FlashPlanConfirmDialogProps) {
  const { flashPartitions, effectiveWipeCount } = useMemo(() => {
    const flashPartitions = selectedPartitions.filter((partition) => partition.action === "flash");
    const visibleWipeCount = selectedPartitions.filter((partition) => partition.action === "wipe").length;
    const includesUserdata = selectedPartitions.some((partition) => partition.partition === "userdata");
    const hiddenCleanFlashWipes =
      plan?.mode === "clean-flash" && includesUserdata
        ? (plan.partitions ?? []).filter(
            (partition) => !partition.user_visible && partition.action === "wipe",
          ).length
        : 0;
    const effectiveWipeCount =
      visibleWipeCount + hiddenCleanFlashWipes;

    return { flashPartitions, effectiveWipeCount };
  }, [plan, selectedPartitions]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(34rem,calc(100vw-1rem))] !max-w-none gap-4 bg-background text-foreground sm:!max-w-none" showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Confirm flash plan</DialogTitle>
        </DialogHeader>

        <div className="grid grid-cols-2 gap-2">
          <SummaryCard label="Mode" value={
            plan ? (
              <Badge variant={plan.mode === "clean-flash" ? "success" : plan.mode === "dirty_flash" ? "secondary" : "outline"} className="h-5 text-[10px] px-1.5 py-0">
                {flashModeLabel(plan.mode.replaceAll("-", "_"))}
              </Badge>
            ) : "flash"
          } />
          <SummaryCard label="Selected" value={`${selectedPartitions.length} partition${selectedPartitions.length === 1 ? "" : "s"}`} />
          <SummaryCard label="Flash" value={
            <Badge variant="success" className="h-5 text-[10px] px-1.5 py-0">
              {flashPartitions.length}
            </Badge>
          } />
          <SummaryCard label="Wipe" value={
            <Badge variant="warning" className="h-5 text-[10px] px-1.5 py-0">
              {effectiveWipeCount}
            </Badge>
          } />
        </div>

        <DialogFooter className="items-stretch sm:items-center">
          <Button variant="outline" onClick={() => onOpenChange(false)} disabled={isPending} className="w-full sm:w-auto">
            Cancel
          </Button>
          <Button onClick={onConfirm} disabled={isPending} className="w-full sm:w-auto">
            {isPending ? "Starting..." : "Flash"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
});

function SummaryCard({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div className="rounded-md border border-border/70 bg-muted/20 px-3 py-2">
      <p className="text-[10px] font-medium uppercase tracking-[0.16em] text-muted-foreground">{label}</p>
      <div className="mt-1 text-sm font-semibold leading-5 text-foreground">{value}</div>
    </div>
  );
}
