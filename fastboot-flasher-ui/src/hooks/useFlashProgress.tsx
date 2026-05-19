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
import type { FlashEvent, FlashSummaryDto, ForceFastbootEvent } from "@/types/api";

export interface FlashProgress {
  phase: "idle" | "waiting" | "flashing" | "complete" | "cancelled" | "error";
  runMode: "" | "live" | "dry_run";
  operation: "" | "flash" | "erase";
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

export function FlashProgressProvider({ children }: { children: ReactNode }) {
  const runModeRef = useRef<FlashProgress["runMode"]>("");
  const isMinimizedRef = useRef(false);
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
    setLogEntries((prev) => [...prev, entry].slice(-100));
  }, []);

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    listen<FlashEvent>("flash-progress", (evt) => {
      if (cancelled) return;
      try {
        const ev = evt.payload;
        const logEntry = formatFlashEventForLog(ev);
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
              errorMessage: "",
            }));
            toast.info("Waiting for device...");
            break;
          case "GsiStatus":
            setState((p) => ({
              ...p,
              statusText: gsiStatusMessage(ev.data.status),
            }));
            break;
          case "PlanBuilt":
            setState((p) => ({
              ...p,
              overallBytes: 0,
              overallTotal: ev.data.total_bytes,
              summary: null,
            }));
            toast.info(`${ev.data.actions} actions, ${(ev.data.total_bytes / 1e9).toFixed(2)} GiB`);
            break;
          case "PreparingImage":
            runModeRef.current = "live";
            setState((p) => ({
              ...p,
              runMode: "live",
              operation: "flash",
              partition: ev.data.partition,
            }));
            break;
          case "Flashing":
            runModeRef.current = "live";
            setState((p) => ({
              ...p,
              phase: "flashing",
              runMode: "live",
              operation: "flash",
              partition: ev.data.partition,
              bytes: ev.data.bytes,
              total: ev.data.total,
              speedBps: ev.data.speed_bps,
              errorMessage: "",
            }));
            break;
          case "Simulating":
            runModeRef.current = "dry_run";
            setState((p) => ({
              ...p,
              phase: "flashing",
              runMode: "dry_run",
              operation: ev.data.action === "wipe" ? "erase" : "flash",
              partition: ev.data.partition,
              bytes: ev.data.bytes,
              total: ev.data.total,
              speedBps: ev.data.speed_bps,
              errorMessage: "",
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
            toast.error(`${ev.data.partition}: ${ev.data.error}`);
            break;
          case "Erasing":
            runModeRef.current = "live";
            setState((p) => ({
              ...p,
              phase: "flashing",
              runMode: "live",
              operation: "erase",
              partition: ev.data.partition,
              bytes: 0,
              total: 1,
            }));
            break;
          case "Complete":
            setState((p) => ({
              ...p,
              phase: "complete",
              summary: ev.data.summary,
              errorMessage: "",
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
            setState((p) => ({
              ...p,
              phase: "cancelled",
              operation: "",
              errorMessage: ev.data.message,
            }));
            toast.message("Flash cancelled");
            tryNotify("Flash cancelled");
            break;
          case "Error":
            setState((p) => ({
              ...p,
              phase: "error",
              errorMessage: ev.data.message,
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
    setLogEntries((prev) => [...prev, `Error ${JSON.stringify({ message })}`].slice(-100));
    setState((prev) => ({
      ...prev,
      phase: "error",
      errorMessage: message,
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

function formatFlashEventForLog(event: FlashEvent): string | null {
  switch (event.event) {
    case "WaitingForDevice":
      return "WaitingForDevice";
    case "GsiStatus":
      return `GsiStatus ${JSON.stringify(event.data)}`;
    case "PlanBuilt":
      return `PlanBuilt ${JSON.stringify(event.data)}`;
    case "PreparingImage":
      return `PreparingImage ${JSON.stringify(event.data)}`;
    case "PartitionComplete":
      return `PartitionComplete ${JSON.stringify(event.data)}`;
    case "PartitionSkipped":
      return `PartitionSkipped ${JSON.stringify(event.data)}`;
    case "PartitionFailed":
      return `PartitionFailed ${JSON.stringify(event.data)}`;
    case "Erasing":
      return `Erasing ${JSON.stringify(event.data)}`;
    case "EraseComplete":
      return `EraseComplete ${JSON.stringify(event.data)}`;
    case "Complete":
      return `Complete ${JSON.stringify(event.data)}`;
    case "Cancelled":
      return `Cancelled ${JSON.stringify(event.data)}`;
    case "Error":
      return `Error ${JSON.stringify(event.data)}`;
    case "Flashing":
    case "Overall":
    case "Simulating":
      return null;
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
      return "Rebooting to fastbootd";
    case "rebooting_to_bootloader":
      return "Rebooting to bootloader";
    case "starting_bootloader_phase":
      return "Starting bootloader phase";
    case "starting_fastbootd_phase":
      return "Starting fastbootd phase";
    case "preparing_vbmeta_flash":
      return "Preparing vbmeta flash";
    case "flashing_vbmeta":
      return "Flashing vbmeta";
    case "checking_system_partition":
      return "Checking system partition";
    case "checking_product_gsi_fallback":
      return "Checking product fallback";
    case "generating_product_gsi_image":
      return "Generating product_gsi image";
    case "flashing_product_gsi":
      return "Flashing product_gsi";
    case "wiping_userdata":
      return "Wiping data";
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
