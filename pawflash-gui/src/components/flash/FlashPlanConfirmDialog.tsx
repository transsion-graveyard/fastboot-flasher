import { memo } from "react";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
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
  const flashPartitions = selectedPartitions.filter((partition) => partition.action === "flash");
  const wipePartitions = selectedPartitions.filter((partition) => partition.action === "wipe");

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="w-[min(34rem,calc(100vw-1rem))] !max-w-none gap-4 bg-background text-foreground sm:!max-w-none" showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Confirm flash plan</DialogTitle>
        </DialogHeader>

        <div className="grid grid-cols-2 gap-2">
          <SummaryCard label="Mode" value={plan ? flashModeLabel(plan.mode.replaceAll("-", "_")) : "flash"} />
          <SummaryCard label="Selected" value={`${selectedPartitions.length} partition${selectedPartitions.length === 1 ? "" : "s"}`} />
          <SummaryCard label="Flash" value={`${flashPartitions.length} partition${flashPartitions.length === 1 ? "" : "s"}`} />
          <SummaryCard label="Wipe" value={`${wipePartitions.length} partition${wipePartitions.length === 1 ? "" : "s"}`} />
        </div>

        {plan?.mode === "clean-flash" ? (
          <p className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-sm text-amber-100">
            Clean flash will remove all files and apps from internal storage.
          </p>
        ) : null}

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

function SummaryCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md border border-border/70 bg-muted/20 px-3 py-2">
      <p className="text-[10px] font-medium uppercase tracking-[0.16em] text-muted-foreground">{label}</p>
      <p className="mt-1 text-sm font-semibold leading-5 text-foreground">{value}</p>
    </div>
  );
}
