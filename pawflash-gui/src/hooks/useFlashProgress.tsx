/* eslint-disable react-refresh/only-export-components */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { listen } from "@tauri-apps/api/event";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { toast } from "sonner";
import type {
  FlashEvent,
  FlashOperation,
  FlashSummaryDto,
  ForceFastbootEvent,
} from "@/types/api";

export interface FlashProgress {
  phase: "idle" | "waiting" | "flashing" | "complete" | "cancelled" | "error";
  runMode: "" | "live" | "dry_run";
  operation: "" | "flash" | "format" | "erase";
  partition: string;
  bytes: number;
  total: number;
  speedBps: number;
  overallBytes: number;
  overallTotal: number;
  summary: FlashSummaryDto | null;
  errorMessage: string;
  statusText: string;
  reset: () => void;
  fail: (message: string) => void;
  setIsMinimized: (v: boolean) => void;
}

export interface FlashLog {
  entries: string[];
  clear: () => void;
  append: (entry: string) => void;
}

const FlashProgressContext = createContext<FlashProgress | null>(null);
const FlashLogContext = createContext<FlashLog | null>(null);
const LOG_RETENTION_LIMIT = 300;
const PROGRESS_LOG_STEP = 10;

export function FlashProgressProvider({ children }: { children: ReactNode }) {
  const runModeRef = useRef<FlashProgress["runMode"]>("");
  const isMinimizedRef = useRef(false);
  const progressMilestonesRef = useRef<Record<string, number>>({});
  const [state, setState] = useState<Omit<FlashProgress, "reset" | "fail" | "setIsMinimized">>({
    phase: "idle",
    runMode: "",
    operation: "",
    partition: "",
    bytes: 0,
    total: 0,
    speedBps: 0,
    overallBytes: 0,
    overallTotal: 0,
    summary: null,
    errorMessage: "",
    statusText: "",
  });
  const [logEntries, setLogEntries] = useState<string[]>([]);

  const appendLogEntry = useCallback((entry: string) => {
    setLogEntries((prev) => [...prev, entry].slice(-LOG_RETENTION_LIMIT));
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    listen<FlashEvent>("flash-progress", (evt) => {
      if (cancelled) return;
      try {
        const ev = evt.payload;
        const logEntry = formatFlashEventForLog(ev, runModeRef.current, progressMilestonesRef.current);
        if (logEntry) {
          appendLogEntry(logEntry);
        }

        switch (ev.event) {
          case "WaitingForDevice":
            runModeRef.current = "live";
            setState((p) => ({
              ...p,
              phase: "waiting",
              runMode: "live",
              operation: "",
              partition: "",
              bytes: 0,
              total: 0,
              speedBps: 0,
              errorMessage: "",
            }));
            toast.info("Waiting for device...");
            break;
          case "DeviceCheckDiagnostic":
            break;
          case "GsiStatus":
            setState((p) => ({
              ...p,
              statusText: "",
              operation: "",
              partition: "",
              bytes: 0,
              total: 0,
              speedBps: 0,
            }));
            toast.info(gsiStatusMessage(ev.data.status));
            break;
          case "Rebooting":
            setState((p) => ({
              ...p,
              statusText: `Rebooting to ${ev.data.target}`,
              operation: "",
              partition: "",
              bytes: 0,
              total: 0,
              speedBps: 0,
            }));
            toast.info(`Rebooting to ${ev.data.target}...`);
            break;
          case "PlanBuilt":
            progressMilestonesRef.current = {};
            setState((p) => ({
              ...p,
              overallBytes: 0,
              overallTotal: ev.data.total_bytes,
              summary: null,
            }));
            toast.info(`${ev.data.actions} actions, ${(ev.data.total_bytes / 1e9).toFixed(2)} GiB`);
            break;
          case "PreparingImage":
            progressMilestonesRef.current = clearProgressMilestonesForPartition(
              progressMilestonesRef.current,
              ev.data.partition,
            );
            if (runModeRef.current !== "dry_run") {
              runModeRef.current = "live";
            }
            setState((p) => ({
              ...p,
              runMode: p.runMode || "live",
              operation: toUiOperation(ev.data.operation),
              partition: ev.data.partition,
              bytes: 0,
              total: 0,
              speedBps: 0,
              statusText: "",
            }));
            toast.info(`Preparing ${ev.data.partition}...`);
            break;
          case "Flashing":
            runModeRef.current = "live";
            setState((p) => ({
              ...p,
              phase: "flashing",
              runMode: "live",
              operation: toUiOperation(ev.data.operation),
              partition: ev.data.partition,
              bytes: ev.data.bytes,
              total: ev.data.total,
              speedBps: ev.data.speed_bps,
              errorMessage: "",
              statusText: "",
            }));
            break;
          case "Simulating":
            runModeRef.current = "dry_run";
            setState((p) => ({
              ...p,
              phase: "flashing",
              runMode: "dry_run",
              operation: toUiOperation(ev.data.operation),
              partition: ev.data.partition,
              bytes: ev.data.bytes,
              total: ev.data.total,
              speedBps: ev.data.speed_bps,
              errorMessage: "",
              statusText: "",
            }));
            break;
          case "Overall":
            setState((p) => ({
              ...p,
              overallBytes: ev.data.bytes,
              overallTotal: ev.data.total,
            }));
            break;
          case "PartitionFailed":
            progressMilestonesRef.current = clearProgressMilestonesForPartition(
              progressMilestonesRef.current,
              ev.data.partition,
            );
            toast.error(`${ev.data.partition}: ${ev.data.error}`);
            break;
          case "PartitionComplete":
            toast.success(completionToast(ev.data.partition, ev.data.operation));
            break;
          case "PartitionSkipped":
            toast.warning(skipToast(ev.data.partition, ev.data.operation));
            break;
          case "EraseComplete":
            toast.success(`${ev.data.partition} erased`);
            break;
          case "Erasing":
            progressMilestonesRef.current = clearProgressMilestonesForPartition(
              progressMilestonesRef.current,
              ev.data.partition,
            );
            runModeRef.current = "live";
            setState((p) => ({
              ...p,
              phase: "flashing",
              runMode: "live",
              operation: "erase",
              partition: ev.data.partition,
              bytes: 0,
              total: 1,
              speedBps: 0,
              statusText: "",
            }));
            toast.info(`Erasing ${ev.data.partition}...`);
            break;
          case "Complete":
            progressMilestonesRef.current = {};
            setState((p) => ({
              ...p,
              phase: "complete",
              summary: ev.data.summary,
              errorMessage: "",
              statusText: "",
              bytes: p.total > 0 ? p.total : p.bytes,
              total: p.total > 0 ? p.total : p.bytes,
              overallBytes: ev.data.summary.total_bytes,
              overallTotal: ev.data.summary.total_bytes,
            }));
            toast.success(preserveCompletionMessage(runModeRef.current));
            if (isMinimizedRef.current) {
              const summary = ev.data.summary;
              const body = `${summary.flash_count} flashed, ${summary.wipe_count} wiped, ${(summary.total_bytes / 1e9).toFixed(2)} GiB`;
              tryNotify(preserveCompletionMessage(runModeRef.current), body);
            }
            break;
          case "Cancelled":
            progressMilestonesRef.current = {};
            setState((p) => ({
              ...p,
              phase: "cancelled",
              operation: "",
              errorMessage: ev.data.message,
              statusText: "",
            }));
            toast.message("Flash cancelled");
            tryNotify("Flash cancelled");
            break;
          case "Error":
            progressMilestonesRef.current = {};
            setState((p) => ({
              ...p,
              phase: "error",
              errorMessage: ev.data.message,
              statusText: "",
            }));
            toast.error(ev.data.message);
            tryNotify("Flash failed", ev.data.message);
            break;
        }
      } catch (error) {
        console.error("flash-progress listener crashed", error, evt.payload);
      }
    }).then((fn) => {
      if (cancelled) {
        fn();
        return;
      }
      unlisten = fn;
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [appendLogEntry]);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    listen<ForceFastbootEvent>("force-fastboot-progress", (evt) => {
      if (cancelled) return;
      const ev = evt.payload;
      switch (ev.event) {
        case "Started":
          appendLogEntry(`ForceFastboot Started ${JSON.stringify(ev.data)}`);
          break;
        case "WaitingForPreloader":
          appendLogEntry(`ForceFastboot WaitingForPreloader ${JSON.stringify(ev.data)}`);
          break;
        case "Complete":
          appendLogEntry(`ForceFastboot Complete ${JSON.stringify(ev.data)}`);
          break;
        case "Cancelled":
          appendLogEntry(`ForceFastboot Cancelled ${JSON.stringify(ev.data)}`);
          break;
        case "Error":
          appendLogEntry(`ForceFastboot Error ${JSON.stringify(ev.data)}`);
          break;
      }
    }).then((fn) => {
      if (cancelled) {
        fn();
        return;
      }
      unlisten = fn;
    });

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [appendLogEntry]);

  const reset = useCallback(() => {
    runModeRef.current = "";
    progressMilestonesRef.current = {};
    setLogEntries([]);
    setState({
      phase: "idle",
      runMode: "",
      operation: "",
      partition: "",
      bytes: 0,
      total: 0,
      speedBps: 0,
      overallBytes: 0,
      overallTotal: 0,
      summary: null,
      errorMessage: "",
      statusText: "",
    });
  }, []);

  const fail = useCallback((message: string) => {
    progressMilestonesRef.current = {};
    setLogEntries((prev) => [...prev, `Error ${JSON.stringify({ message })}`].slice(-LOG_RETENTION_LIMIT));
    setState((prev) => ({
      ...prev,
      phase: "error",
      errorMessage: message,
      statusText: "",
    }));
  }, []);

  const setIsMinimized = useCallback((v: boolean) => {
    isMinimizedRef.current = v;
  }, []);

  const clear = useCallback(() => {
    setLogEntries([]);
  }, []);

  const progressValue = useMemo(
    () => ({ ...state, reset, fail, setIsMinimized }) satisfies FlashProgress,
    [state, reset, fail, setIsMinimized],
  );

  const logValue = useMemo(
    () => ({ entries: logEntries, clear, append: appendLogEntry }) satisfies FlashLog,
    [appendLogEntry, clear, logEntries],
  );

  return (
    <FlashProgressContext.Provider value={progressValue}>
      <FlashLogContext.Provider value={logValue}>
        {children}
      </FlashLogContext.Provider>
    </FlashProgressContext.Provider>
  );
}

export function useFlashProgress() {
  const ctx = useContext(FlashProgressContext);
  if (!ctx) throw new Error("useFlashProgress must be used within FlashProgressProvider");
  return ctx;
}

export function useFlashLog() {
  const ctx = useContext(FlashLogContext);
  if (!ctx) throw new Error("useFlashLog must be used within FlashProgressProvider");
  return ctx;
}

async function tryNotify(title: string, body?: string) {
  try {
    const permitted = await isPermissionGranted();
    if (!permitted) {
      const permission = await requestPermission();
      if (permission !== "granted") return;
    }
    sendNotification({ title, body });
  } catch {
    // Notification permission denied or unavailable on this platform
  }
}

function preserveCompletionMessage(runMode: FlashProgress["runMode"]) {
  return runMode === "dry_run" ? "Dry run complete" : "Flash complete";
}

function formatFlashEventForLog(
  event: FlashEvent,
  runMode: FlashProgress["runMode"],
  progressMilestones: Record<string, number>,
): string | null {
  switch (event.event) {
    case "WaitingForDevice":
      return "WaitingForDevice Waiting for fastboot device";
    case "DeviceCheckDiagnostic":
      return formatDeviceCheckDiagnostic(event.data.stage, event.data.level, event.data.message);
    case "GsiStatus":
      return `GsiPhase ${gsiStatusMessage(event.data.status)}`;
    case "Rebooting":
      return `Rebooting target=${event.data.target}`;
    case "PlanBuilt":
      return `PlanBuilt actions=${event.data.actions} total=${formatGiB(event.data.total_bytes)}`;
    case "PreparingImage":
      return runMode === "dry_run"
        ? `DryRunPrepare operation=${event.data.operation} partition=${event.data.partition}`
        : `FlashPrepare operation=${event.data.operation} partition=${event.data.partition}`;
    case "Flashing":
      return formatProgressMilestone({
        prefix: event.data.operation === "format_userdata" ? "FormatProgress" : "FlashProgress",
        label: "partition",
        partition: event.data.partition,
        bytes: event.data.bytes,
        total: event.data.total,
        speedBps: event.data.speed_bps,
        progressMilestones,
      });
    case "Simulating":
      return formatProgressMilestone({
        prefix:
          event.data.operation === "erase"
            ? "DryRunEraseProgress"
            : event.data.operation === "format_userdata"
              ? "DryRunFormatProgress"
              : "DryRunProgress",
        label: event.data.operation,
        partition: event.data.partition,
        bytes: event.data.bytes,
        total: event.data.total,
        speedBps: event.data.speed_bps,
        progressMilestones,
      });
    case "Overall":
      return null;
    case "PartitionComplete":
      return `PartitionComplete operation=${event.data.operation} partition=${event.data.partition}`;
    case "PartitionSkipped":
      return `PartitionSkipped operation=${event.data.operation} partition=${event.data.partition} reason=${event.data.reason}`;
    case "PartitionFailed":
      return `PartitionFailed operation=${event.data.operation} partition=${event.data.partition} error=${event.data.error}`;
    case "Erasing":
      return runMode === "dry_run"
        ? `DryRunEraseStart partition=${event.data.partition}`
        : `Erasing partition=${event.data.partition}`;
    case "EraseComplete":
      return `EraseComplete partition=${event.data.partition}`;
    case "Complete":
      return `Complete flashed=${event.data.summary.flash_count} wiped=${event.data.summary.wipe_count} skipped=${event.data.summary.skipped_count} total=${formatGiB(event.data.summary.total_bytes)}`;
    case "Cancelled":
      return `Cancelled message=${event.data.message}`;
    case "Error":
      return `Error message=${event.data.message}`;
  }
}

function formatProgressMilestone({
  prefix,
  label,
  partition,
  bytes,
  total,
  speedBps,
  progressMilestones,
}: {
  prefix: string;
  label: string;
  partition: string;
  bytes: number;
  total: number;
  speedBps: number;
  progressMilestones: Record<string, number>;
}) {
  if (total <= 0) {
    return null;
  }

  const key = `${prefix}:${partition}`;
  const pct = Math.min(100, Math.floor((bytes / total) * 100));
  const milestone = progressMilestoneBucket(pct);
  if (milestone === null) {
    return null;
  }
  if (progressMilestones[key] === milestone) {
    return null;
  }

  progressMilestones[key] = milestone;
  const parts = [
    prefix,
    `${label}=${partition}`,
    `progress=${milestone}%`,
    `${formatBytes(bytes)} / ${formatBytes(total)}`,
  ];
  if (speedBps > 0) {
    parts.push(`speed=${formatBytes(speedBps)}/s`);
  }
  return parts.join(" ");
}

function progressMilestoneBucket(pct: number) {
  if (pct <= 0) return 0;
  if (pct >= 100) return 100;
  return Math.floor(pct / PROGRESS_LOG_STEP) * PROGRESS_LOG_STEP;
}

function toUiOperation(operation: FlashOperation): FlashProgress["operation"] {
  switch (operation) {
    case "erase":
      return "erase";
    case "format_userdata":
      return "format";
    default:
      return "flash";
  }
}

function completionToast(partition: string, operation: FlashOperation) {
  if (operation === "format_userdata") {
    return `${partition} formatted`;
  }
  return `${partition} complete`;
}

function skipToast(partition: string, operation: FlashOperation) {
  if (operation === "format_userdata") {
    return `${partition} format skipped`;
  }
  return `${partition} skipped`;
}

function clearProgressMilestonesForPartition(
  progressMilestones: Record<string, number>,
  partition: string,
) {
  return Object.fromEntries(
    Object.entries(progressMilestones).filter(([key]) => !key.endsWith(`:${partition}`)),
  );
}

function formatBytes(bytes: number) {
  if (!Number.isFinite(bytes) || bytes <= 0) {
    return "0 B";
  }

  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const precision = unitIndex === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2;
  return `${value.toFixed(precision)} ${units[unitIndex]}`;
}

function formatGiB(bytes: number) {
  return `${(bytes / 1e9).toFixed(2)} GiB`;
}

function formatDeviceCheckDiagnostic(stage: string, level: string, message: string) {
  const prefix = level === "error"
    ? "DeviceProbeError"
    : level === "warning"
      ? "DeviceProbeWarning"
      : "DeviceProbe";

  return `${prefix} ${humanizeProbeStage(stage)} ${message}`;
}

function humanizeProbeStage(stage: string) {
  switch (stage) {
    case "using_cached_device":
      return "Using cached fastboot device";
    case "enumerating":
      return "Enumerating fastboot devices";
    case "candidate_found":
      return "Found fastboot interface candidate";
    case "open_ok":
      return "Opened fastboot interface";
    case "open_failed":
      return "Opening fastboot interface failed";
    case "no_interface_yet":
      return "No fastboot interface detected yet";
    case "retrying":
      return "Waiting before next device probe";
    case "reading_vars":
      return "Reading fastboot variables";
    case "read_vars_ok":
      return "Read fastboot variables";
    case "read_vars_failed":
      return "Reading fastboot variables failed";
    case "mode_detected":
      return "Detected fastboot mode";
    case "refreshing_connection":
      return "Refreshing fastboot connection";
    case "probe_failed":
      return "Fastboot probe failed";
    default:
      return stage;
  }
}

function gsiStatusMessage(status: string) {
  switch (status) {
    case "bootloader_detected":
      return "Bootloader detected";
    case "fastbootd_detected":
      return "Fastbootd detected";
    case "bootloader_ready":
      return "Bootloader ready";
    case "fastbootd_ready":
      return "Fastbootd ready";
    case "rebooting_to_fastbootd":
      return "Rebooting into fastbootd";
    case "waiting_for_fastbootd":
      return "Waiting for fastbootd";
    case "rebooting_to_bootloader":
      return "Rebooting into bootloader";
    case "starting_bootloader_phase":
      return "Starting bootloader phase";
    case "starting_fastbootd_phase":
      return "Starting fastbootd phase";
    case "preparing_vbmeta_flash":
      return "Preparing empty vbmeta";
    case "flashing_vbmeta":
      return "Flashing empty vbmeta";
    case "checking_system_partition":
      return "Checking system partition";
    case "checking_product_gsi_fallback":
      return "Checking product fallback";
    case "generating_product_gsi_image":
      return "Generating product_gsi image";
    case "flashing_product_gsi":
      return "Flashing product_gsi";
    case "wiping_userdata":
      return "Wiping userdata";
    case "flashing_system_gsi":
      return "Flashing system GSI";
    case "product_gsi_fallback_not_needed":
      return "Product fallback not needed";
    case "userdata_erase_fallback":
      return "Using userdata erase fallback";
    case "gsi_flow_complete":
      return "GSI flow complete";
    default:
      return `Running GSI action: ${status}`;
  }
}
