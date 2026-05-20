import { ScatterPicker } from "@/components/main-tab/ScatterPicker";
import { FlashOptions } from "@/components/main-tab/FlashOptions";
import { PartitionTable } from "@/components/main-tab/PartitionTable";
import { FlashFab } from "@/components/main-tab/FlashFab";
import { AlertTriangle, XCircle } from "lucide-react";
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
    <div className="flex h-full min-h-0 flex-col gap-4">
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
        <div className="shrink-0 space-y-2 text-sm text-muted-foreground">
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
      <div className="panel-shell grid shrink-0 gap-4 px-4 py-4 xl:grid-cols-[minmax(0,1fr)_auto] xl:items-center">
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
          <SummaryCard label="Chipset" value={plan?.chipset ?? "—"} />
          <SummaryCard label="Storage" value={plan?.storage ?? "—"} />
          <SummaryCard label="Slot" value={plan?.slot_policy ?? "—"} />
          <SummaryCard
            label="Actions"
            value={plan ? `F = ${selectedSummary.flashCount} / W = ${selectedSummary.wipeCount}` : isParsingPlan ? "Parsing..." : "—"}
            accent
          />
        </div>
        <div className="xl:text-right">
          <FlashFab onClick={() => setFlashConfirmOpen(true)} disabled={flashDisabled} />
        </div>
      </div>
    </div>
  );
}

function SummaryCard({
  label,
  value,
  accent = false,
}: {
  label: string;
  value: string;
  accent?: boolean;
}) {
  return (
    <div className="panel-inset flex h-12 flex-col justify-center px-3">
      <p className="text-[10px] leading-tight font-medium uppercase tracking-[0.12em] text-muted-foreground">{label}</p>
      <p className={accent ? "mt-0.5 text-sm leading-tight font-semibold text-accent-soft-foreground" : "mt-0.5 text-sm leading-tight font-semibold"}>
        {value}
      </p>
    </div>
  );
}
