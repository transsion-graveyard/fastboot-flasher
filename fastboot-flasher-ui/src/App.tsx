import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Toaster, toast } from "sonner";
import { AlertTriangle, LoaderCircle, MonitorUp, PlugZap, XCircle } from "lucide-react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import { AppLayout } from "@/components/layout/AppLayout";
import { ScatterPicker } from "@/components/main-tab/ScatterPicker";
import { FlashOptions } from "@/components/main-tab/FlashOptions";
import { PartitionTable } from "@/components/main-tab/PartitionTable";
import { FlashFab } from "@/components/main-tab/FlashFab";
import { GsiFlasher } from "@/components/extra-tab/GsiFlasher";
import { FlashDialog } from "@/components/flash/FlashDialog";
import { ForceFastbootDialog } from "@/components/flash/ForceFastbootDialog";
import { DeviceSection } from "@/components/menu-tab/DeviceSection";
import { BootloaderSection } from "@/components/menu-tab/BootloaderSection";
import { DataSection } from "@/components/menu-tab/DataSection";
import { LogSection } from "@/components/menu-tab/LogSection";
import { RebootSection } from "@/components/menu-tab/RebootSection";
import { SlotSection } from "@/components/menu-tab/SlotSection";
import { useDevice } from "@/hooks/useDevice";
import { useFlashLog, useFlashProgress } from "@/hooks/useFlashProgress";
import { useForceFastboot } from "@/hooks/useForceFastboot";
import { defaultFlashMode, type FlashMode } from "@/lib/flash-mode";
import type { FlashPlanDto, ParseScatterResponseDto, PartitionDto } from "@/types/api";

const SCATTER_STORAGE_KEY = "last-scatter-path";
const GSI_STORAGE_KEY = "last-gsi-image-path";
const DEVICE_CHECK_TIMEOUT_MS = 120_000;
const DEVICE_CHECK_TIMEOUT_LABEL = "2 minutes";
type AppTheme = "light" | "dark";

function resolveInitialTheme(): AppTheme {
  if (typeof window === "undefined") {
    return "light";
  }

  const media = window.matchMedia("(prefers-color-scheme: dark)");
  const resolvedSystemTheme: AppTheme = media.matches ? "dark" : "light";
  const stored = window.localStorage.getItem("app-theme");
  if (stored === "light" || stored === "dark") {
    return stored;
  }
  if (stored === "system") {
    return resolvedSystemTheme;
  }
  return resolvedSystemTheme;
}

function partitionSortRank(partition: PartitionDto): number {
  if (partition.action !== "flash") {
    return 2;
  }

  return partition.image_path ? 0 : 1;
}

export default function App() {
  const [scatterPath, setScatterPath] = useState(() => {
    if (typeof window === "undefined") {
      return "";
    }
    return window.localStorage.getItem(SCATTER_STORAGE_KEY) ?? "";
  });
  const [scatterReloadToken, setScatterReloadToken] = useState(0);
  const [mode, setMode] = useState<FlashMode>(defaultFlashMode);
  const [theme, setTheme] = useState<AppTheme>(resolveInitialTheme);
  const [rebootAfter, setRebootAfter] = useState(false);
  const [advanced, setAdvanced] = useState(false);
  const [includePreloader, setIncludePreloader] = useState(false);
  const [slot, setSlot] = useState<"" | "a" | "b" | "all">("");
  const [plan, setPlan] = useState<FlashPlanDto | null>(null);
  const [planId, setPlanId] = useState<number | null>(null);
  const [partitions, setPartitions] = useState<PartitionDto[]>([]);
  const [imageOverrides, setImageOverrides] = useState<Record<string, string>>({});
  const [flashOpen, setFlashOpen] = useState(false);
  const [flashMinimized, setFlashMinimized] = useState(false);
  const [forceOpen, setForceOpen] = useState(false);
  const [forceMinimized, setForceMinimized] = useState(false);
  const [isCancellingFlash, setIsCancellingFlash] = useState(false);
  const [isCancellingForceFastboot, setIsCancellingForceFastboot] = useState(false);
  const [isStartingFlash, setIsStartingFlash] = useState(false);
  const [isStartingGsiFlash, setIsStartingGsiFlash] = useState(false);
  const [isFormattingData, setIsFormattingData] = useState(false);
  const [isParsingPlan, setIsParsingPlan] = useState(false);
  const [isCheckingDevice, setIsCheckingDevice] = useState(false);
  const [gsiImagePath, setGsiImagePath] = useState(() => {
    if (typeof window === "undefined") {
      return "";
    }
    return window.localStorage.getItem(GSI_STORAGE_KEY) ?? "";
  });
  const planRequestRef = useRef(0);
  const lastParsedScatterPathRef = useRef("");

  const device = useDevice();
  const flash = useFlashProgress();
  const { append: appendLog } = useFlashLog();
  const forceFastboot = useForceFastboot();

  const handleModeChange = useCallback(
    (newMode: string) => {
      appendLog(`ModeChanged ${newMode}`);
      setMode(newMode as FlashMode);
    },
    [appendLog],
  );

  const handleRebootChange = useCallback(
    (value: boolean) => {
      appendLog(`RebootAfter ${value ? "on" : "off"}`);
      setRebootAfter(value);
    },
    [appendLog],
  );

  const handleAdvancedChange = useCallback(
    (value: boolean) => {
      appendLog(`AdvancedMode ${value ? "on" : "off"}`);
      setAdvanced(value);
    },
    [appendLog],
  );

  const handleIncludePreloaderChange = useCallback(
    (value: boolean) => {
      appendLog(`IncludePreloader ${value ? "on" : "off"}`);
      setIncludePreloader(value);
    },
    [appendLog],
  );

  const handleSlotChange = useCallback(
    (newSlot: "" | "a" | "b" | "all") => {
      appendLog(`SlotOverride ${newSlot || "default"}`);
      setSlot(newSlot);
    },
    [appendLog],
  );

  const refreshPlan = useCallback(
    async (
      path: string,
      selectedMode: string,
      selectedAdvanced: boolean,
      selectedIncludePreloader: boolean,
      selectedSlot: "" | "a" | "b" | "all",
    ) => {
      const requestId = ++planRequestRef.current;

      try {
        const response = await invoke<ParseScatterResponseDto>("parse_scatter", {
          path,
          mode: selectedMode,
          slot: selectedAdvanced && selectedSlot ? selectedSlot : null,
          includePreloader: selectedAdvanced ? selectedIncludePreloader : false,
        });

        if (planRequestRef.current !== requestId) {
          return;
        }

        setIsParsingPlan(true);
        setPlanId(null);
        appendLog("ParseStart");

        const dto = response.plan;
        setImageOverrides((prev) => {
          const visible = new Set(dto.partitions.map((partition) => partition.partition));
          return Object.fromEntries(
            Object.entries(prev).filter(([partition]) => visible.has(partition)),
          );
        });
        appendLog(`ParseComplete ${dto.partitions.length} partitions`);
        setPlanId(response.plan_id);
        setPlan(dto);
        setPartitions((prev) => {
          const preserveSelection = lastParsedScatterPathRef.current === path;
          const selection = preserveSelection
            ? new Map(prev.map((p) => [p.partition, p.selected]))
            : new Map<string, boolean>();
          return dto.partitions.map((p) => ({
            ...p,
            selected: selection.get(p.partition) ?? p.selected,
          }));
        });
        lastParsedScatterPathRef.current = path;
      } catch (error) {
        if (planRequestRef.current !== requestId) {
          return;
        }

        setIsParsingPlan(true);
        appendLog("ParseStart");
        appendLog(`ParseError ${String(error)}`);
        setPlan(null);
        setPlanId(null);
        setPartitions([]);
        setImageOverrides({});
        lastParsedScatterPathRef.current = "";
        window.localStorage.removeItem(SCATTER_STORAGE_KEY);
        toast.error(String(error));
      } finally {
        if (planRequestRef.current === requestId) {
          setIsParsingPlan(false);
        }
      }
    },
    [appendLog],
  );

  useEffect(() => {
    if (!scatterPath) {
      return;
    }

    const timeoutId = window.setTimeout(() => {
      void refreshPlan(scatterPath, mode, advanced, includePreloader, slot);
    }, 0);

    return () => {
      window.clearTimeout(timeoutId);
    };
  }, [advanced, includePreloader, mode, refreshPlan, scatterPath, scatterReloadToken, slot]);

  useEffect(() => {
    if (typeof window === "undefined") {
      return;
    }

    if (gsiImagePath) {
      window.localStorage.setItem(GSI_STORAGE_KEY, gsiImagePath);
    } else {
      window.localStorage.removeItem(GSI_STORAGE_KEY);
    }
  }, [gsiImagePath]);

  useEffect(() => {
    if (flash.phase === "complete" || flash.phase === "cancelled" || flash.phase === "error") {
      const timeoutId = window.setTimeout(() => {
        setIsCancellingFlash(false);
        setFlashMinimized(false);
      }, 0);

      return () => {
        window.clearTimeout(timeoutId);
      };
    }
  }, [flash.phase]);

  useEffect(() => {
    if (forceFastboot.phase === "complete" || forceFastboot.phase === "cancelled" || forceFastboot.phase === "error") {
      const timeoutId = window.setTimeout(() => {
        setIsCancellingForceFastboot(false);
        setForceMinimized(false);
      }, 0);

      return () => {
        window.clearTimeout(timeoutId);
      };
    }
  }, [forceFastboot.phase]);

  const loadScatter = useCallback(
    (path: string) => {
      appendLog("ScatterLoad");
      setPlan(null);
      setPlanId(null);
      setPartitions([]);
      setImageOverrides({});
      setScatterPath(path);
      setScatterReloadToken((token) => token + 1);
      window.localStorage.setItem(SCATTER_STORAGE_KEY, path);
    },
    [appendLog],
  );

  const togglePartition = useCallback(
    (index: number) => {
      const partition = partitions[index];
      if (partition) {
        appendLog(`PartitionToggled ${partition.partition} ${!partition.selected ? "selected" : "deselected"}`);
      }
      setPartitions((prev) => prev.map((p, i) => (i === index ? { ...p, selected: !p.selected } : p)));
    },
    [appendLog, partitions],
  );

  const toggleAllPartitions = useCallback(() => {
    const nextSelected = !partitions.every((p) => p.selected);
    appendLog(`PartitionsAllToggled ${nextSelected ? "selected" : "cleared"}`);
    setPartitions((prev) => prev.map((p) => ({ ...p, selected: nextSelected })));
  }, [appendLog, partitions]);

  const startFlash = useCallback(async () => {
    if (
      isStartingFlash ||
      isParsingPlan ||
      planId === null ||
      forceFastboot.phase === "waiting"
    ) {
      return;
    }

    const selected = partitions.filter((p) => p.selected).map((p) => p.partition);
    appendLog(`FlashStarted ${selected.length} partitions`);
    flash.reset();
    setFlashOpen(true);
    setFlashMinimized(false);
    setIsStartingFlash(true);

    try {
      await invoke("start_flash", {
        planId,
        partitions: selected,
        imageOverrides,
        reboot: rebootAfter,
      });
    } catch (error) {
      flash.fail(String(error));
    } finally {
      setIsStartingFlash(false);
    }
  }, [
    appendLog,
    flash,
    forceFastboot.phase,
    imageOverrides,
    isParsingPlan,
    isStartingFlash,
    partitions,
    planId,
    rebootAfter,
  ]);

  const startForceFastboot = useCallback(async () => {
    if (flash.phase === "waiting" || flash.phase === "flashing" || forceFastboot.phase === "waiting") {
      return;
    }

    forceFastboot.reset();
    setForceOpen(true);
    setForceMinimized(false);
    appendLog("ForceFastboot StartRequested");

    try {
      await forceFastboot.start();
    } catch (error) {
      const message = String(error);
      appendLog(`ForceFastboot StartError ${message}`);
      toast.error(message);
      forceFastboot.reset();
      setForceOpen(false);
    }
  }, [appendLog, flash.phase, forceFastboot]);

  const activeFlashSession = flash.phase === "waiting" || flash.phase === "flashing";
  const activeForceSession = forceFastboot.phase === "waiting";
  const menuActionDisabled =
    isStartingFlash ||
    isStartingGsiFlash ||
    isFormattingData ||
    isCheckingDevice ||
    activeFlashSession ||
    activeForceSession;

  const startGsiFlash = useCallback(async () => {
    if (
      !gsiImagePath ||
      isStartingGsiFlash ||
      isStartingFlash ||
      isFormattingData ||
      isCheckingDevice ||
      activeFlashSession ||
      activeForceSession
    ) {
      return;
    }

    flash.reset();
    setFlashOpen(true);
    setFlashMinimized(false);
    setIsStartingGsiFlash(true);
    appendLog(`GsiFlashStarted ${gsiImagePath.split(/[/\\]/).pop() || gsiImagePath}`);

    try {
      await invoke("start_gsi_flash", {
        image: gsiImagePath,
      });
    } catch (error) {
      flash.fail(String(error));
    } finally {
      setIsStartingGsiFlash(false);
    }
  }, [
    activeFlashSession,
    activeForceSession,
    appendLog,
    flash,
    gsiImagePath,
    isCheckingDevice,
    isFormattingData,
    isStartingFlash,
    isStartingGsiFlash,
  ]);

  const startWipeData = useCallback(async () => {
    if (menuActionDisabled) {
      return;
    }

    flash.reset();
    setFlashOpen(true);
    setFlashMinimized(false);
    setIsFormattingData(true);
    appendLog("WipeData StartRequested");

    try {
      await invoke("wipe_data", {
        noMetadata: false,
        noCache: false,
        eraseFallback: false,
      });
    } catch (error) {
      const message = String(error);
      appendLog(`WipeData Error ${message}`);
      flash.fail(message);
    } finally {
      setIsFormattingData(false);
    }
  }, [appendLog, flash, menuActionDisabled]);

  const checkDevice = useCallback(async () => {
    if (isCheckingDevice || activeFlashSession || activeForceSession) {
      return;
    }

    setIsCheckingDevice(true);
    appendLog("DeviceCheck Started");

    try {
      const timeoutPromise = new Promise<never>((_, reject) =>
        setTimeout(
          () =>
            reject(new Error(`Device check timed out after ${DEVICE_CHECK_TIMEOUT_LABEL}`)),
          DEVICE_CHECK_TIMEOUT_MS,
        )
      );
      const info = await Promise.race([device.check(), timeoutPromise]);
      const summary = `serial=${info.serial || "unknown"} product=${info.product || "unknown"} slot=${info.slot || "unknown"} unlocked=${info.unlocked || "unknown"}`;
      appendLog(`DeviceCheck Connected ${summary}`);
      toast.success(`Connected: ${info.serial || info.product || "device"}`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      appendLog(`DeviceCheck Error ${message}`);
      toast.error(message);
    } finally {
      setIsCheckingDevice(false);
    }
  }, [activeFlashSession, activeForceSession, appendLog, device, isCheckingDevice]);

  const pickCustomImage = useCallback(
    async (partition: PartitionDto) => {
      const selected = await open({
        title: `Select image for ${partition.partition}`,
        multiple: false,
      });

      if (typeof selected !== "string") {
        return;
      }

      const name = selected.split(/[/\\]/).pop() || selected;
      appendLog(`CustomImagePicked ${partition.partition} ${name}`);
      setImageOverrides((prev) => ({
        ...prev,
        [partition.partition]: selected,
      }));
      toast.success(`Custom image set for ${partition.partition}`);
    },
    [appendLog],
  );

  const hideFlashDialog = useCallback(() => {
    setFlashOpen(false);
    setFlashMinimized(activeFlashSession);
  }, [activeFlashSession]);

  const hideForceDialog = useCallback(() => {
    setForceOpen(false);
    setForceMinimized(activeForceSession);
  }, [activeForceSession]);

  const cancelFlash = useCallback(async () => {
    if (!activeFlashSession || isCancellingFlash) {
      return;
    }

    appendLog("FlashCancelled");
    setIsCancellingFlash(true);
    setFlashOpen(false);
    setFlashMinimized(false);

    try {
      await invoke("cancel_flash");
    } catch (error) {
      setIsCancellingFlash(false);
      flash.fail(String(error));
    }
  }, [activeFlashSession, appendLog, flash, isCancellingFlash]);

  const cancelForceFastboot = useCallback(async () => {
    if (!activeForceSession || isCancellingForceFastboot) {
      return;
    }

    appendLog("ForceFastboot Cancelled");
    setIsCancellingForceFastboot(true);
    setForceOpen(false);
    setForceMinimized(false);

    try {
      await forceFastboot.cancel();
    } catch (error) {
      setIsCancellingForceFastboot(false);
      toast.error(String(error));
    }
  }, [activeForceSession, appendLog, forceFastboot, isCancellingForceFastboot]);

  const displayPartitions = useMemo(
    () =>
      partitions
        .map((partition) => {
        const overridePath = imageOverrides[partition.partition];
        const overrideName = overridePath?.split(/[/\\]/).pop() ?? null;
        return {
          ...partition,
          image_path: overridePath ?? partition.image_path,
          image_name: overrideName ?? partition.image_name,
          image_overridden: Boolean(overridePath),
        };
        })
        .sort((left, right) => {
          const rankDiff = partitionSortRank(left) - partitionSortRank(right);
          if (rankDiff !== 0) {
            return rankDiff;
          }
          return left.index - right.index;
        }),
    [imageOverrides, partitions],
  );

  const selectedPartitions = useMemo(
    () => displayPartitions.filter((partition) => partition.selected),
    [displayPartitions],
  );

  const selectedSummary = useMemo(
    () => ({
      flashCount: selectedPartitions.filter((partition) => partition.action === "flash").length,
      wipeCount: selectedPartitions.filter((partition) => partition.action === "wipe").length,
    }),
    [selectedPartitions],
  );

  const allPartitionsSelected = partitions.length > 0 && partitions.every((partition) => partition.selected);
  const somePartitionsSelected = partitions.some((partition) => partition.selected) && !allPartitionsSelected;

  const flashDisabled =
    !plan ||
    planId === null ||
    isParsingPlan ||
    isStartingFlash ||
    isStartingGsiFlash ||
    activeFlashSession ||
    activeForceSession ||
    selectedPartitions.length === 0;

  const sidebarStatus = (
    <div className="space-y-3">
      {activeFlashSession && (
        <div className="status-shell min-w-0 space-y-2 px-3 py-3">
          <div className="flex min-w-0 items-center gap-2 text-sm">
            <span className="inline-block h-2.5 w-2.5 shrink-0 rounded-full bg-accent-brand" />
            <span className="truncate text-muted-foreground">{phaseLabel(flash.phase, flash.runMode, flash.operation)}</span>
          </div>
          {flashMinimized && (
            <Button
              variant="outline"
              size="sm"
              className="w-full justify-start gap-2 overflow-hidden"
              onClick={() => setFlashOpen(true)}
            >
              <MonitorUp className="h-4 w-4" />
              <span className="truncate">Show progress</span>
            </Button>
          )}
        </div>
      )}

      {activeForceSession && (
        <div className="status-shell min-w-0 space-y-2 px-3 py-3">
          <div className="flex min-w-0 items-center gap-2 text-sm text-muted-foreground">
            <LoaderCircle className="h-4 w-4 shrink-0 animate-spin" />
            <span className="truncate">Waiting for preloader...</span>
          </div>
          {forceMinimized && (
            <Button
              variant="outline"
              size="sm"
              className="w-full justify-start gap-2 overflow-hidden"
              onClick={() => setForceOpen(true)}
            >
              <MonitorUp className="h-4 w-4" />
              <span className="truncate">Show progress</span>
            </Button>
          )}
        </div>
      )}
    </div>
  );

  const sidebarActions = (
    <Button
      variant="outline"
      size="sm"
      className="w-full justify-start gap-2 overflow-hidden"
      disabled={isCheckingDevice || activeFlashSession || activeForceSession}
      onClick={checkDevice}
    >
      <PlugZap className="h-4 w-4 shrink-0" />
      <span className="truncate">{isCheckingDevice ? "Checking device..." : "Check Device"}</span>
    </Button>
  );

  return (
    <TooltipProvider>
      <Toaster richColors position="top-center" theme={theme} />
      <AppLayout
        sidebarStatus={sidebarStatus}
        sidebarActions={sidebarActions}
        theme={theme}
        onThemeChange={setTheme}
      >
        {({ tab }) =>
          tab === "main" ? (
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
                  <FlashFab onClick={startFlash} disabled={flashDisabled} />
                </div>
              </div>
            </div>
          ) : tab === "extra" ? (
            <div className="flex h-full min-h-0 flex-col gap-5">
              <GsiFlasher
                imagePath={gsiImagePath}
                onImagePathChange={setGsiImagePath}
                onFlash={startGsiFlash}
                disabled={menuActionDisabled}
                flashing={isStartingGsiFlash}
              />
            </div>
          ) : (
            <div className="grid h-full min-h-0 gap-5 xl:grid-cols-[minmax(0,0.9fr)_minmax(0,1.1fr)]">
              <div className="flex min-h-0 flex-col gap-5">
                <DeviceSection
                  onForceFastboot={startForceFastboot}
                  forceFastbootDisabled={menuActionDisabled}
                  disableVbmetaDisabled={menuActionDisabled}
                  disabled={menuActionDisabled}
                />
                <BootloaderSection />
                <DataSection
                  onWipeData={startWipeData}
                  disabled={menuActionDisabled}
                />
                <SlotSection disabled={menuActionDisabled} />
              </div>
              <div className="flex min-h-0 flex-col gap-5">
                <RebootSection disabled={menuActionDisabled} />
                <LogSection />
              </div>
            </div>
          )
        }
      </AppLayout>
      <FlashDialog
        open={flashOpen}
        onOpenChange={(nextOpen) => {
          if (!nextOpen) {
            hideFlashDialog();
            return;
          }
          setFlashOpen(true);
        }}
        onMinimize={hideFlashDialog}
        onCancel={cancelFlash}
        canCancel={activeFlashSession}
      />
      <ForceFastbootDialog
        open={forceOpen}
        onOpenChange={(nextOpen) => {
          if (!nextOpen) {
            hideForceDialog();
            return;
          }
          setForceOpen(true);
        }}
        onHide={hideForceDialog}
        onCancel={cancelForceFastboot}
      />
    </TooltipProvider>
  );
}

function phaseLabel(
  phase: "idle" | "waiting" | "flashing" | "complete" | "cancelled" | "error",
  runMode: "" | "live" | "dry_run",
  operation: "" | "flash" | "erase",
) {
  if (phase === "waiting") return "Waiting for device...";
  if (phase === "flashing" && runMode === "dry_run") {
    return operation === "erase" ? "Dry run erase..." : "Dry run...";
  }
  if (phase === "flashing") {
    return operation === "erase" ? "Erasing..." : "Flashing...";
  }
  if (phase === "cancelled") return "Cancelled";
  if (phase === "complete") return runMode === "dry_run" ? "Dry run complete" : "Complete";
  if (phase === "error") return "Error";
  return "";
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
