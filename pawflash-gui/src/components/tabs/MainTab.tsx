import { ScatterPicker } from "@/components/main-tab/ScatterPicker";
import { FlashOptions } from "@/components/main-tab/FlashOptions";
import { PartitionTable } from "@/components/main-tab/PartitionTable";
import { FlashFab } from "@/components/main-tab/FlashFab";
import { cn } from "@/lib/utils";
import { AlertTriangle, XCircle } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import type { FlashPlanDto, PartitionDto } from "@/types/api";
import type { FlashMode } from "@/lib/flash-mode";

interface MainTabProps {
  scatterPath: string;
  loadScatter: (path: string) => void;
  mode: FlashMode;
  handleModeChange: (mode: string) => void;
  rebootAfter: boolean;
  handleRebootChange: (v: boolean) => void;
  advanced: boolean;
  handleAdvancedChange: (v: boolean) => void;
  includePreloader: boolean;
  handleIncludePreloaderChange: (v: boolean) => void;
  slot: "" | "a" | "b" | "active" | "inactive" | "all";
  handleSlotChange: (slot: "" | "a" | "b" | "active" | "inactive" | "all") => void;
  displayPartitions: PartitionDto[];
  isParsingPlan: boolean;
  togglePartition: (index: number) => void;
  toggleAllPartitions: () => void;
  allPartitionsSelected: boolean;
  somePartitionsSelected: boolean;
  pickCustomImage: (partition: PartitionDto) => void;
  plan: FlashPlanDto | null;
  selectedSummary: { flashCount: number; wipeCount: number };
  flashDisabled: boolean;
  setFlashConfirmOpen: (open: boolean) => void;
}

export function MainTab({
  scatterPath,
  loadScatter,
  mode,
  handleModeChange,
  rebootAfter,
  handleRebootChange,
  advanced,
  handleAdvancedChange,
  includePreloader,
  handleIncludePreloaderChange,
  slot,
  handleSlotChange,
  displayPartitions,
  isParsingPlan,
  togglePartition,
  toggleAllPartitions,
  allPartitionsSelected,
  somePartitionsSelected,
  pickCustomImage,
  plan,
  selectedSummary,
  flashDisabled,
  setFlashConfirmOpen,
}: MainTabProps) {
  return (
    <div className="flex h-full min-h-0 flex-col gap-4 lg:gap-6">
      <ScatterPicker path={scatterPath} onChange={loadScatter} />
      <FlashOptions
        mode={mode}
        onModeChange={handleModeChange}
        reboot={rebootAfter}
        onRebootChange={handleRebootChange}
        advanced={advanced}
        onAdvancedChange={handleAdvancedChange}
        includePreloader={includePreloader}
        onIncludePreloaderChange={handleIncludePreloaderChange}
        slot={slot}
        onSlotChange={handleSlotChange}
      />
      <PartitionTable
        className="min-h-0 flex-1"
        partitions={displayPartitions}
        isParsingPlan={isParsingPlan}
        onToggle={togglePartition}
        onToggleAll={toggleAllPartitions}
        allSelected={allPartitionsSelected}
        someSelected={somePartitionsSelected}
        onPickImage={pickCustomImage}
      />
      {advanced && plan && (
        <div className="shrink-0 space-y-2 px-2 text-sm text-muted-foreground">
          {plan.warnings.map((warning, index) => (
            <p key={index} className="flex items-start gap-2 leading-6 text-warning">
              <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
              {warning}
            </p>
          ))}
          {plan.errors.map((error, index) => (
            <p key={index} className="flex items-start gap-2 leading-6 text-error">
              <XCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
              {error}
            </p>
          ))}
        </div>
      )}
      <div className="panel-shell shrink-0 px-5 py-4 sm:px-6 sm:py-5">
        <div className="grid grid-cols-6 items-center gap-3 lg:grid-cols-5 lg:gap-4">
          <SummaryCard label="Chipset" value={plan?.chipset ?? "—"} className="col-span-2 lg:col-span-1" />
          <SummaryCard label="Storage" value={plan?.storage ?? "—"} className="col-span-2 lg:col-span-1" />
          <SummaryCard label="Slot" value={plan?.slot_policy ?? "—"} className="col-span-2 lg:col-span-1" />
          <SummaryCard
            label="Actions"
            value={plan ? (
              <span className="inline-flex items-center gap-1.5">
                <Badge variant="success" className="px-2 py-0">
                  F {selectedSummary.flashCount}
                </Badge>
                <Badge variant="warning" className="px-2 py-0">
                  W {selectedSummary.wipeCount}
                </Badge>
              </span>
            ) : isParsingPlan ? "Parsing..." : "—"}
            accent
            className="col-span-3 lg:col-span-1"
          />
          <div className="col-span-3 lg:col-span-1 flex overflow-hidden">
            <FlashFab onClick={() => setFlashConfirmOpen(true)} disabled={flashDisabled} />
          </div>
        </div>
      </div>
    </div>
  );
}

function SummaryCard({
  label,
  value,
  accent = false,
  className,
}: {
  label: string;
  value: React.ReactNode;
  accent?: boolean;
  className?: string;
}) {
  return (
    <div className={cn("panel-inset flex h-12 flex-col justify-center gap-0.5 px-3", className)}>
      <p className="text-[11px] leading-tight font-medium uppercase tracking-[0.12em] text-muted-foreground">{label}</p>
      <div className={accent ? "text-sm leading-tight font-semibold text-accent-soft-foreground" : "text-sm leading-tight font-semibold"}>
        {value}
      </div>
    </div>
  );
}
