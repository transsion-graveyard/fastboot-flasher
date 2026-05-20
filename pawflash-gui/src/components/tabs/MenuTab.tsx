import { DeviceSection } from "@/components/menu-tab/DeviceSection";
import { BootloaderSection } from "@/components/menu-tab/BootloaderSection";
import { DataSection } from "@/components/menu-tab/DataSection";
import { SlotSection } from "@/components/menu-tab/SlotSection";
import { RebootSection, type RebootTarget } from "@/components/menu-tab/RebootSection";
import { LogSection } from "@/components/menu-tab/LogSection";

interface MenuTabProps {
  onForceFastboot: () => void;
  menuActionDisabled: boolean;
  onFormatData: () => void;
  rebootTarget: RebootTarget;
  onRebootTargetChange: (target: RebootTarget) => void;
}

export function MenuTab({
  onForceFastboot,
  menuActionDisabled,
  onFormatData,
  rebootTarget,
  onRebootTargetChange,
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
          <DataSection
            onFormatData={onFormatData}
            disabled={menuActionDisabled}
          />
          <SlotSection disabled={menuActionDisabled} />
        </div>
      </div>
      <div className="flex min-h-0 flex-col gap-2">
        <RebootSection
          disabled={menuActionDisabled}
          target={rebootTarget}
          onTargetChange={onRebootTargetChange}
        />
        <LogSection />
      </div>
    </div>
  );
}
