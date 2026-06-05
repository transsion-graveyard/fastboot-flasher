import { DeviceSection } from "@/components/menu-tab/DeviceSection";
import { BootloaderSection } from "@/components/menu-tab/BootloaderSection";
import { SlotSection } from "@/components/menu-tab/SlotSection";
import { LogSection } from "@/components/menu-tab/LogSection";

interface MenuTabProps {
  onForceFastboot: () => void;
  menuActionDisabled: boolean;
}

export function MenuTab({
  onForceFastboot,
  menuActionDisabled,
}: MenuTabProps) {
  return (
    <div className="flex min-h-full min-h-0 flex-col gap-3 lg:grid lg:grid-cols-2 lg:gap-4">
      <div className="flex flex-col gap-3">
        <div className="flex flex-col gap-2">
          <DeviceSection
            onForceFastboot={onForceFastboot}
            forceFastbootDisabled={menuActionDisabled}
            disableVbmetaDisabled={menuActionDisabled}
            disabled={menuActionDisabled}
          />
          <BootloaderSection />
        </div>
        <div className="flex flex-col gap-2">
          <SlotSection disabled={menuActionDisabled} />
        </div>
      </div>
      <div className="flex min-h-0 flex-col gap-2">
        <LogSection />
      </div>
    </div>
  );
}
