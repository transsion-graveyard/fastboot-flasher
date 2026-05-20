import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { Toaster, toast } from "sonner";
import { LoaderCircle, MonitorUp, PlugZap } from "lucide-react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import { AppLayout } from "@/components/layout/AppLayout";
import { FlashDialog } from "@/components/flash/FlashDialog";
import { FlashPlanConfirmDialog } from "@/components/flash/FlashPlanConfirmDialog";
import { ForceFastbootDialog } from "@/components/flash/ForceFastbootDialog";
import type { RebootTarget } from "@/components/menu-tab/RebootSection";
import { useDevice } from "@/hooks/useDevice";
import { useFlashLog, useFlashProgress } from "@/hooks/useFlashProgress";
import { useForceFastboot } from "@/hooks/useForceFastboot";
import { applyDismissibleDialogChange } from "@/components/shared/dialogBehavior";
import { defaultFlashMode, type FlashMode } from "@/lib/flash-mode";
import type { DeviceInfo, FlashPlanDto, ParseScatterResponseDto, PartitionDto } from "@/types/api";

const MainTab = lazy(() => import("@/components/tabs/MainTab").then((m) => ({ default: m.MainTab })));
const ExtraTab = lazy(() => import("@/components/tabs/ExtraTab").then((m) => ({ default: m.ExtraTab })));
const MenuTab = lazy(() => import("@/components/tabs/MenuTab").then((m) => ({ default: m.MenuTab })));

const SCATTER_STORAGE_KEY = "last-scatter-path";
const GSI_STORAGE_KEY = "last-gsi-image-path";
const REBOOT_TARGET_STORAGE_KEY = "last-reboot-target";
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

function buildDeviceSummary(info: DeviceInfo) {
  return [
    `serial=${info.serial || "unknown"}`,
    `product=${info.product || "unknown"}`,
    `slot=${info.slot || "unknown"}`,
    `unlocked=${info.unlocked || "unknown"}`,
    `mode=${info.mode || "bootloader"}`,
  ].join(" ");
}

function appendParsedPlanLog(appendLog: (entry: string) => void, plan: FlashPlanDto) {
  appendLog(
    `ParseSummary mode=${plan.mode} storage=${plan.storage} slot=${plan.slot_policy} chipset=${plan.chipset ?? "unknown"}`,
  );

  plan.partitions.forEach((partition) => {
    appendLog(
      [
        "ParsePartition",
        `action=${partition.action}`,
        `name=${partition.partition}`,
        `size=${partition.size_human}`,
        `image=${partition.image_name ?? partition.image_path ?? "unresolved"}`,
        `source=${partition.source}`,
        `selected=${partition.selected ? "yes" : "no"}`,
      ].join(" "),
    );
  });

  plan.warnings.forEach((warning) => appendLog(`ParseWarning ${warning}`));
  plan.errors.forEach((error) => appendLog(`ParseError ${error}`));
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
  const [slot, setSlot] = useState<"" | "a" | "b" | "active" | "inactive" | "all">("");
  const [plan, setPlan] = useState<FlashPlanDto | null>(null);
  const [planId, setPlanId] = useState<number | null>(null);
  const [partitions, setPartitions] = useState<PartitionDto[]>([]);
  const [imageOverrides, setImageOverrides] = useState<Record<string, string>>({});
  const [flashConfirmOpen, setFlashConfirmOpen] = useState(false);
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
  const [rebootTarget, setRebootTarget] = useState<RebootTarget>(() => {
    if (typeof window === "undefined") {
      return "system";
    }

    const stored = window.localStorage.getItem(REBOOT_TARGET_STORAGE_KEY);
    return stored === "system" || stored === "bootloader" || stored === "fastboot" || stored === "recovery"
      ? stored
      : "system";
  });
  const [gsiImagePath, setGsiImagePath] = useState(() => {
    if (typeof window === "undefined") {
      return "";
    }
    return window.localStorage.getItem(GSI_STORAGE_KEY) ?? "";
  });
  const planRequestRef = useRef(0);
  const lastParsedScatterPathRef = useRef("");
  const partitionsRef = useRef(partitions);
  partitionsRef.current = partitions;

  const device = useDevice();
  const flash = useFlashProgress();
  const { append: appendLog } = useFlashLog();
  const forceFastboot = useForceFastboot();

  const flashPhaseRef = useRef(flash.phase);
  flashPhaseRef.current = flash.phase;
  const forcePhaseRef = useRef(forceFastboot.phase);
  forcePhaseRef.current = forceFastboot.phase;

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
    (newSlot: "" | "a" | "b" | "active" | "inactive" | "all") => {
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
      selectedSlot: "" | "a" | "b" | "active" | "inactive" | "all",
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
        appendParsedPlanLog(appendLog, dto);
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
    if (typeof window === "undefined") {
      return;
    }

    window.localStorage.setItem(REBOOT_TARGET_STORAGE_KEY, rebootTarget);
  }, [rebootTarget]);

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
      const partition = partitionsRef.current[index];
      if (partition) {
        appendLog(`PartitionToggled ${partition.partition} ${!partition.selected ? "selected" : "deselected"}`);
      }
      setPartitions((prev) => prev.map((p, i) => (i === index ? { ...p, selected: !p.selected } : p)));
    },
    [appendLog],
  );

  const toggleAllPartitions = useCallback(() => {
    const nextSelected = !partitionsRef.current.every((p) => p.selected);
    appendLog(`PartitionsAllToggled ${nextSelected ? "selected" : "cleared"}`);
    setPartitions((prev) => prev.map((p) => ({ ...p, selected: nextSelected })));
  }, [appendLog]);

  const startFlash = useCallback(async () => {
    if (
      isStartingFlash ||
      isParsingPlan ||
      planId === null ||
      forceFastboot.phase === "waiting"
    ) {
      return;
    }

    const selected = partitions
      .filter((partition) => partition.user_visible && partition.selected)
      .map((partition) => partition.partition);
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
    const sessionLive = flashPhaseRef.current === "waiting" || flashPhaseRef.current === "flashing";
    if (sessionLive || forcePhaseRef.current === "waiting") {
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
  }, [appendLog, forceFastboot]);

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
    const sessionLive = flashPhaseRef.current === "waiting" || flashPhaseRef.current === "flashing";
    const forceLive = forcePhaseRef.current === "waiting";
    if (
      !gsiImagePath ||
      isStartingGsiFlash ||
      isStartingFlash ||
      isFormattingData ||
      isCheckingDevice ||
      sessionLive ||
      forceLive
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
    appendLog,
    flash,
    gsiImagePath,
    isCheckingDevice,
    isFormattingData,
    isStartingFlash,
    isStartingGsiFlash,
  ]);

  const readVariable = useCallback(
    async (name: string) => {
      appendLog(`Getvar Requested ${name}`);
      return device.getVariable(name);
    },
    [appendLog, device],
  );

  const readAllVariables = useCallback(async () => {
    appendLog("GetvarAll Requested");
    return device.getAllVariables();
  }, [appendLog, device]);

  const startManualFlash = useCallback(
    async (
      partition: string,
      image: string,
      selectedSlot: "" | "a" | "b" | "active" | "inactive" | "all",
    ) => {
      const sessionLive = flashPhaseRef.current === "waiting" || flashPhaseRef.current === "flashing";
      const forceLive = forcePhaseRef.current === "waiting";
      if (
        isStartingFlash ||
        isStartingGsiFlash ||
        isFormattingData ||
        isCheckingDevice ||
        sessionLive ||
        forceLive
      ) {
        return;
      }

      flash.reset();
      setFlashOpen(true);
      setFlashMinimized(false);
      setIsStartingFlash(true);

      try {
        await invoke("manual_flash", {
          partition,
          image,
          slot: selectedSlot || null,
        });
      } catch (error) {
        flash.fail(String(error));
        throw error;
      } finally {
        setIsStartingFlash(false);
      }
    },
    [
      flash,
      isCheckingDevice,
      isFormattingData,
      isStartingFlash,
      isStartingGsiFlash,
    ],
  );

  const startFormatData = useCallback(async () => {
    const sessionLive = flashPhaseRef.current === "waiting" || flashPhaseRef.current === "flashing";
    const forceLive = forcePhaseRef.current === "waiting";
    const anyBusy =
      isStartingFlash ||
      isStartingGsiFlash ||
      isFormattingData ||
      isCheckingDevice ||
      sessionLive ||
      forceLive;
    if (anyBusy) {
      return;
    }

    flash.reset();
    setFlashOpen(true);
    setFlashMinimized(false);
    setIsFormattingData(true);
    appendLog("FormatData StartRequested");

    try {
      await invoke("format_data", {
        noMetadata: false,
        noCache: false,
        eraseFallback: false,
      });
    } catch (error) {
      const message = String(error);
      appendLog(`FormatData Error ${message}`);
      flash.fail(message);
    } finally {
      setIsFormattingData(false);
    }
  }, [appendLog, flash, isCheckingDevice, isFormattingData, isStartingFlash, isStartingGsiFlash]);

  const checkDevice = useCallback(async () => {
    const sessionLive = flashPhaseRef.current === "waiting" || flashPhaseRef.current === "flashing";
    const forceLive = forcePhaseRef.current === "waiting";
    if (isCheckingDevice || sessionLive || forceLive) {
      return;
    }

    setIsCheckingDevice(true);
    appendLog("DeviceCheck Started");

    try {
      const info = await device.check();
      const summary = buildDeviceSummary(info);
      appendLog(`DeviceCheck Connected ${summary}`);
      toast.success(`Connected: ${info.serial || info.product || "device"}`);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      appendLog(`DeviceCheck Error ${message}`);
      toast.error(message);
    } finally {
      setIsCheckingDevice(false);
    }
  }, [appendLog, device, isCheckingDevice]);

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
    setFlashMinimized(flashPhaseRef.current === "waiting" || flashPhaseRef.current === "flashing");
  }, []);

  const hideForceDialog = useCallback(() => {
    setForceOpen(false);
    setForceMinimized(forcePhaseRef.current === "waiting");
  }, []);

  const cancelFlash = useCallback(async () => {
    const sessionLive = flashPhaseRef.current === "waiting" || flashPhaseRef.current === "flashing";
    if (!sessionLive || isCancellingFlash) {
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
  }, [appendLog, flash, isCancellingFlash]);

  const cancelForceFastboot = useCallback(async () => {
    const forceLive = forcePhaseRef.current === "waiting";
    if (!forceLive || isCancellingForceFastboot) {
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
  }, [appendLog, forceFastboot, isCancellingForceFastboot]);

  const displayPartitions = useMemo(
    () =>
      partitions
        .filter((partition) => partition.user_visible)
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

  const allPartitionsSelected =
    displayPartitions.length > 0 && displayPartitions.every((partition) => partition.selected);
  const somePartitionsSelected =
    displayPartitions.some((partition) => partition.selected) && !allPartitionsSelected;

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
        {({ tab }) => (
          <Suspense fallback={null}>
            {tab === "main" && (
              <MainTab
                scatterPath={scatterPath}
                loadScatter={loadScatter}
                mode={mode}
                handleModeChange={handleModeChange}
                rebootAfter={rebootAfter}
                handleRebootChange={handleRebootChange}
                advanced={advanced}
                handleAdvancedChange={handleAdvancedChange}
                includePreloader={includePreloader}
                handleIncludePreloaderChange={handleIncludePreloaderChange}
                slot={slot}
                handleSlotChange={handleSlotChange}
                displayPartitions={displayPartitions}
                isParsingPlan={isParsingPlan}
                togglePartition={togglePartition}
                toggleAllPartitions={toggleAllPartitions}
                allPartitionsSelected={allPartitionsSelected}
                somePartitionsSelected={somePartitionsSelected}
                pickCustomImage={pickCustomImage}
                plan={plan}
                selectedSummary={selectedSummary}
                flashDisabled={flashDisabled}
                setFlashConfirmOpen={setFlashConfirmOpen}
              />
            )}
            {tab === "extra" && (
              <ExtraTab
                gsiImagePath={gsiImagePath}
                onGsiImagePathChange={setGsiImagePath}
                onGsiFlash={startGsiFlash}
                menuActionDisabled={menuActionDisabled}
                isStartingGsiFlash={isStartingGsiFlash}
                onManualFlash={startManualFlash}
                isStartingFlash={isStartingFlash}
                rebootTarget={rebootTarget}
                onRebootTargetChange={setRebootTarget}
                onGetVariable={readVariable}
                onGetAllVariables={readAllVariables}
              />
            )}
            {tab === "menu" && (
              <MenuTab
                onForceFastboot={startForceFastboot}
                menuActionDisabled={menuActionDisabled}
                onFormatData={startFormatData}
                rebootTarget={rebootTarget}
                onRebootTargetChange={setRebootTarget}
              />
            )}
          </Suspense>
        )}
      </AppLayout>
      <FlashPlanConfirmDialog
        open={flashConfirmOpen}
        onOpenChange={setFlashConfirmOpen}
        onConfirm={async () => {
          setFlashConfirmOpen(false);
          await startFlash();
        }}
        plan={plan}
        selectedPartitions={selectedPartitions}
        isPending={isStartingFlash || isParsingPlan || flash.phase === "waiting" || forceFastboot.phase === "waiting"}
        rebootAfter={rebootAfter}
      />
      <FlashDialog
        open={flashOpen}
        onOpenChange={(nextOpen, reason) => {
          applyDismissibleDialogChange(nextOpen, reason, hideFlashDialog, () => setFlashOpen(true));
        }}
        onMinimize={hideFlashDialog}
        onCancel={cancelFlash}
        canCancel={activeFlashSession}
      />
      <ForceFastbootDialog
        open={forceOpen}
        onOpenChange={(nextOpen, reason) => {
          applyDismissibleDialogChange(nextOpen, reason, hideForceDialog, () => setForceOpen(true));
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
  operation: "" | "flash" | "format" | "erase",
) {
  if (phase === "waiting") return "Waiting for device...";
  if (phase === "flashing" && runMode === "dry_run") {
    if (operation === "erase") return "Dry run erase...";
    if (operation === "format") return "Dry run format...";
    return "Dry run...";
  }
  if (phase === "flashing") {
    if (operation === "erase") return "Erasing...";
    if (operation === "format") return "Formatting...";
    return "Flashing...";
  }
  if (phase === "cancelled") return "Cancelled";
  if (phase === "complete") return runMode === "dry_run" ? "Dry run complete" : "Complete";
  if (phase === "error") return "Error";
  return "";
}
