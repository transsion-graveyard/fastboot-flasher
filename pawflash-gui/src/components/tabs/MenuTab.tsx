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
    <div className="grid h-full min-h-0 gap-5 xl:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)]">
      <div className="flex min-h-0 flex-col gap-5">
        <DeviceSection
          onForceFastboot={onForceFastboot}
          forceFastbootDisabled={menuActionDisabled}
          disableVbmetaDisabled={menuActionDisabled}
          disabled={menuActionDisabled}
        />
        <BootloaderSection />
        <DataSection
          onFormatData={onFormatData}
          disabled={menuActionDisabled}
        />
        <SlotSection disabled={menuActionDisabled} />
      </div>
      <div className="flex min-h-0 flex-col gap-5">
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
