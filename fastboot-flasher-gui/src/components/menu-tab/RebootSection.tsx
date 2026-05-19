import { useMemo, useState } from "react";
import { toast } from "sonner";
import { Power, RotateCcw } from "lucide-react";
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

type RebootTarget = "system" | "bootloader" | "fastboot" | "recovery" | "poweroff";

const targetLabels: Record<RebootTarget, string> = {
  system: "System",
  bootloader: "Bootloader",
  fastboot: "Fastbootd",
  recovery: "Recovery",
  poweroff: "Power off",
};

const successLabels: Record<RebootTarget, string> = {
  system: "Rebooted to system",
  bootloader: "Rebooted to bootloader",
  fastboot: "Rebooted to fastbootd",
  recovery: "Rebooted to recovery",
  poweroff: "Powered off device",
};

interface RebootSectionProps {
  disabled?: boolean;
}

export function RebootSection({ disabled = false }: RebootSectionProps) {
  const { reboot, rebootBootloader, rebootFastboot, rebootRecovery, powerOff } = useDevice();
  const [target, setTarget] = useState<RebootTarget>("system");
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
      case "poweroff":
        return powerOff;
      default:
        return reboot;
    }
  }, [powerOff, reboot, rebootBootloader, rebootFastboot, rebootRecovery, target]);

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
        <Select value={target} onValueChange={(value) => setTarget(value as RebootTarget)}>
          <SelectTrigger className="w-full" aria-label="Reboot target" disabled={disabled || busy}>
            <SelectValue>{targetLabels[target]}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="system">System</SelectItem>
            <SelectItem value="bootloader">Bootloader</SelectItem>
            <SelectItem value="fastboot">Fastbootd</SelectItem>
            <SelectItem value="recovery">Recovery</SelectItem>
            <SelectItem value="poweroff">Power off</SelectItem>
          </SelectContent>
        </Select>
        <Button
          variant="outline"
          className="gap-3"
          disabled={disabled || busy}
          onClick={handleReboot}
        >
          {target === "poweroff" ? <Power className="h-4 w-4" /> : <RotateCcw className="h-4 w-4" />}
          {busy
            ? "Sending command..."
            : target === "poweroff"
              ? "Power off device"
              : `Reboot to ${targetLabels[target]}`}
        </Button>
    </SectionCard>
  );
}
