import { memo, useMemo, useState } from "react";
import { toast } from "sonner";
import { RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { useDevice } from "@/hooks/useDevice";
import { SectionCard } from "@/components/menu-tab/SectionCard";
import { useFlashLog } from "@/hooks/useFlashProgress";

export type RebootTarget = "system" | "bootloader" | "fastboot" | "recovery";

const targetLabels: Record<RebootTarget, string> = {
  system: "System",
  bootloader: "Bootloader",
  fastboot: "Fastbootd",
  recovery: "Recovery",
};

const successLabels: Record<RebootTarget, string> = {
  system: "Rebooted to system",
  bootloader: "Rebooted to bootloader",
  fastboot: "Rebooted to fastbootd",
  recovery: "Rebooted to recovery",
};

interface RebootSectionProps {
  disabled?: boolean;
  target: RebootTarget;
  onTargetChange: (target: RebootTarget) => void;
}

export const RebootSection = memo(function RebootSection({ disabled = false, target, onTargetChange }: RebootSectionProps) {
  const { reboot, rebootBootloader, rebootFastboot, rebootRecovery } = useDevice();
  const [busy, setBusy] = useState(false);
  const { append } = useFlashLog();

  const action = useMemo(() => {
    switch (target) {
      case "bootloader":
        return rebootBootloader;
      case "fastboot":
        return rebootFastboot;
      case "recovery":
        return rebootRecovery;
      default:
        return reboot;
    }
  }, [reboot, rebootBootloader, rebootFastboot, rebootRecovery, target]);

  const handleReboot = async () => {
    setBusy(true);
    append(`RebootTo ${targetLabels[target]} Started`);
    try {
      await action();
      append(`RebootTo ${targetLabels[target]} Complete`);
      toast.success(successLabels[target]);
    } catch (error) {
      append(`RebootTo ${targetLabels[target]} Error ${error}`);
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SectionCard
      title="Reboot"
      contentClassName="grid gap-3 sm:grid-cols-[minmax(0,1fr)_auto]"
    >
      <Select value={target} onValueChange={(value) => onTargetChange(value as RebootTarget)}>
        <SelectTrigger className="w-full" aria-label="Reboot target" disabled={disabled || busy}>
          <SelectValue>{targetLabels[target]}</SelectValue>
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="system">System</SelectItem>
          <SelectItem value="bootloader">Bootloader</SelectItem>
          <SelectItem value="fastboot">Fastbootd</SelectItem>
          <SelectItem value="recovery">Recovery</SelectItem>
        </SelectContent>
      </Select>
      <Button
        variant="outline"
        className="gap-3"
        disabled={disabled || busy}
        onClick={handleReboot}
      >
        <RotateCcw className="h-4 w-4" />
        {busy ? "Sending command..." : `Reboot to ${targetLabels[target]}`}
      </Button>
    </SectionCard>
  );
});
