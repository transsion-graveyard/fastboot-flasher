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
    <div className="flex min-h-full min-h-0 flex-col gap-5 lg:grid lg:grid-cols-2 lg:gap-6">
      <div className="flex flex-col gap-4">
        <GsiFlasher
          imagePath={gsiImagePath}
          onImagePathChange={onGsiImagePathChange}
          onFlash={onGsiFlash}
          disabled={menuActionDisabled}
          flashing={isStartingGsiFlash}
        />
        <ManualFlash
          disabled={menuActionDisabled}
          flashing={isStartingFlash}
          onManualFlash={onManualFlash}
        />
      </div>
      <div className="flex flex-col gap-4">
        <RebootSection
          variant="flat"
          disabled={menuActionDisabled}
          target={rebootTarget}
          onTargetChange={onRebootTargetChange}
        />
        <FastbootVars
          variant="flat"
          disabled={menuActionDisabled}
          onGetVariable={onGetVariable}
          onGetAllVariables={onGetAllVariables}
          className="flex-1 min-h-0"
        />
      </div>
    </div>
  );
}
