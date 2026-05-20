import { GsiFlasher } from "@/components/extra-tab/GsiFlasher";
import { ManualFlash } from "@/components/extra-tab/ManualFlash";
import { FastbootVars } from "@/components/extra-tab/FastbootVars";
import { RebootSection, type RebootTarget } from "@/components/menu-tab/RebootSection";

interface ExtraTabProps {
  gsiImagePath: string;
  onGsiImagePathChange: (path: string) => void;
  onGsiFlash: () => void;
  menuActionDisabled: boolean;
  isStartingGsiFlash: boolean;
  onManualFlash: (
    partition: string,
    image: string,
    slot: "" | "a" | "b" | "active" | "inactive" | "all",
  ) => Promise<void>;
  isStartingFlash: boolean;
  rebootTarget: RebootTarget;
  onRebootTargetChange: (target: RebootTarget) => void;
  onGetVariable: (name: string) => Promise<string>;
  onGetAllVariables: () => Promise<Record<string, string>>;
}

export function ExtraTab({
  gsiImagePath,
  onGsiImagePathChange,
  onGsiFlash,
  menuActionDisabled,
  isStartingGsiFlash,
  onManualFlash,
  isStartingFlash,
  rebootTarget,
  onRebootTargetChange,
  onGetVariable,
  onGetAllVariables,
}: ExtraTabProps) {
  return (
    <div className="grid h-full min-h-0 gap-5 lg:grid-cols-2">
      <div className="lg:col-start-1 lg:row-start-1">
        <GsiFlasher
          imagePath={gsiImagePath}
          onImagePathChange={onGsiImagePathChange}
          onFlash={onGsiFlash}
          disabled={menuActionDisabled}
          flashing={isStartingGsiFlash}
        />
      </div>
      <div className="lg:col-start-1 lg:row-start-2">
        <ManualFlash
          disabled={menuActionDisabled}
          flashing={isStartingFlash}
          onManualFlash={onManualFlash}
        />
      </div>
      <div className="lg:col-start-2 lg:row-start-1">
        <RebootSection
          disabled={menuActionDisabled}
          target={rebootTarget}
          onTargetChange={onRebootTargetChange}
        />
      </div>
      <div className="lg:col-start-2 lg:row-start-2">
        <FastbootVars
          disabled={menuActionDisabled}
          onGetVariable={onGetVariable}
          onGetAllVariables={onGetAllVariables}
        />
      </div>
    </div>
  );
}
