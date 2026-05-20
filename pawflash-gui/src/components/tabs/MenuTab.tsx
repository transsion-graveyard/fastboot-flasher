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
    <div className="grid h-full min-h-0 auto-rows-auto gap-5 lg:grid-cols-2">
      <div className="lg:col-start-1 lg:row-start-1">
        <DeviceSection
          onForceFastboot={onForceFastboot}
          forceFastbootDisabled={menuActionDisabled}
          disableVbmetaDisabled={menuActionDisabled}
          disabled={menuActionDisabled}
        />
      </div>
      <div className="lg:col-start-1 lg:row-start-2">
        <BootloaderSection />
      </div>
      <div className="lg:col-start-1 lg:row-start-3">
        <DataSection
          onFormatData={onFormatData}
          disabled={menuActionDisabled}
        />
      </div>
      <div className="lg:col-start-1 lg:row-start-4">
        <SlotSection disabled={menuActionDisabled} />
      </div>
      <div className="lg:col-start-2 lg:row-start-1">
        <RebootSection
          disabled={menuActionDisabled}
          target={rebootTarget}
          onTargetChange={onRebootTargetChange}
        />
      </div>
      <div className="h-full lg:col-start-2 lg:row-start-2 lg:row-span-3">
        <LogSection />
      </div>
    </div>
  );
}
