import { memo } from "react";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import type { PartitionDto } from "@/types/api";

interface FlashPlanConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void | Promise<void>;
  selectedPartitions: PartitionDto[];
  rebootRecoveryAfter: boolean;
  isPending?: boolean;
}

export const FlashPlanConfirmDialog = memo(function FlashPlanConfirmDialog({
  open,
  onOpenChange,
  onConfirm,
  selectedPartitions,
  rebootRecoveryAfter,
  isPending = false,
}: FlashPlanConfirmDialogProps) {
  const rebootNotice = rebootRecoveryAfter
    ? "The device will reboot into recovery after flashing completes."
    : "";
  const flashCount = selectedPartitions.filter((partition) => partition.action === "flash").length;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(34rem,calc(100vw-1rem))] !max-w-none gap-4 bg-background text-foreground sm:!max-w-none" showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Confirm flash plan</DialogTitle>
        </DialogHeader>

        <div className="grid grid-cols-2 gap-2">
          <SummaryCard label="Selected" value={`${selectedPartitions.length} partition${selectedPartitions.length === 1 ? "" : "s"}`} />
          <SummaryCard label="Flash" value={`${flashCount} partition${flashCount === 1 ? "" : "s"}`} />
        </div>

        {rebootNotice && (
          <p className="rounded-md border border-border/70 bg-muted/20 px-3 py-2 text-sm text-muted-foreground">
            {rebootNotice}
          </p>
        )}

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
