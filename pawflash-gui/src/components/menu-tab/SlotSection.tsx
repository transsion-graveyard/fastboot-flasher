import { useState } from "react";
import { ArrowRightLeft } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { SectionCard } from "@/components/menu-tab/SectionCard";
import { useDevice } from "@/hooks/useDevice";
import { useFlashLog } from "@/hooks/useFlashProgress";

interface SlotSectionProps {
  disabled?: boolean;
}

export function SlotSection({ disabled = false }: SlotSectionProps) {
  const { setActiveSlot } = useDevice();
  const { append } = useFlashLog();
  const [busySlot, setBusySlot] = useState<"a" | "b" | null>(null);

  const applySlot = async (slot: "a" | "b") => {
    setBusySlot(slot);
    append(`SetActiveSlot ${slot.toUpperCase()} Started`);

    try {
      await setActiveSlot(slot);
      append(`SetActiveSlot ${slot.toUpperCase()} Complete`);
      toast.success(`Active slot set to ${slot.toUpperCase()}`);
    } catch (error) {
      append(`SetActiveSlot ${slot.toUpperCase()} Error ${error}`);
      toast.error(String(error));
    } finally {
      setBusySlot(null);
    }
  };

  return (
    <SectionCard title="Slot" contentClassName="grid grid-cols-2 gap-3">
      <Button
        variant="outline"
        className="w-full gap-3"
        disabled={disabled || busySlot !== null}
        onClick={() => void applySlot("a")}
      >
        <ArrowRightLeft className="h-4 w-4" />
        {busySlot === "a" ? "Setting..." : "Set slot A"}
      </Button>
      <Button
        variant="outline"
        className="w-full gap-3"
        disabled={disabled || busySlot !== null}
        onClick={() => void applySlot("b")}
      >
        <ArrowRightLeft className="h-4 w-4" />
        {busySlot === "b" ? "Setting..." : "Set slot B"}
      </Button>
    </SectionCard>
  );
}
