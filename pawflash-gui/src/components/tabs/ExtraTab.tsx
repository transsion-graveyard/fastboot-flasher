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
    <div className="grid h-full min-h-0 gap-5 xl:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)]">
      <div className="flex min-h-0 flex-col gap-5">
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
      <div className="flex min-h-0 flex-col gap-5">
        <RebootSection
          disabled={menuActionDisabled}
          target={rebootTarget}
          onTargetChange={onRebootTargetChange}
        />
        <FastbootVars
          disabled={menuActionDisabled}
          onGetVariable={onGetVariable}
          onGetAllVariables={onGetAllVariables}
        />
      </div>
    </div>
  );
}
