/* eslint-disable react-refresh/only-export-components */
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { toast } from "sonner";
import type { ForceFastbootEvent, ForceFastbootStartDto } from "@/types/api";

interface ForceFastbootState {
  phase: "idle" | "waiting" | "complete" | "cancelled" | "error";
  sessionId: number | null;
  message: string;
  start: () => Promise<void>;
  cancel: () => Promise<void>;
  reset: () => void;
}

const ForceFastbootContext = createContext<ForceFastbootState | null>(null);

export function ForceFastbootProvider({ children }: { children: ReactNode }) {
  const [phase, setPhase] = useState<ForceFastbootState["phase"]>("idle");
  const [sessionId, setSessionId] = useState<number | null>(null);
  const [message, setMessage] = useState("");

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;

    listen<ForceFastbootEvent>("force-fastboot-progress", (event) => {
      if (cancelled) return;
      const payload = event.payload;
      if (payload.event !== "Started" && sessionId !== null && payload.data.session_id !== sessionId) {
        return;
      }

      switch (payload.event) {
        case "Started":
          setSessionId(payload.data.session_id);
          setPhase("waiting");
          setMessage("");
          break;
        case "WaitingForPreloader":
          setPhase("waiting");
          setMessage("");
          break;
        case "Complete":
          setPhase("complete");
          setMessage("");
          setSessionId(null);
          toast.success("Force fastboot complete");
          break;
        case "Cancelled":
          setPhase("cancelled");
          setMessage("");
          setSessionId(null);
          toast.message("Force fastboot cancelled");
          break;
        case "Error":
          setPhase("error");
          setMessage(payload.data.message);
          setSessionId(null);
          toast.error(payload.data.message);
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
  }, [sessionId]);

  const start = useCallback(async () => {
    setPhase("waiting");
    setMessage("");
    const response = await invoke<ForceFastbootStartDto>("start_force_fastboot");
    setSessionId(response.session_id);
  }, []);

  const cancel = useCallback(async () => {
    if (sessionId === null) {
      return;
    }

    await invoke("cancel_force_fastboot", { sessionId });
  }, [sessionId]);

  const reset = useCallback(() => {
    setPhase("idle");
    setSessionId(null);
    setMessage("");
  }, []);

  const value = useMemo(
    () =>
      ({
        phase,
        sessionId,
        message,
        start,
        cancel,
        reset,
      }) satisfies ForceFastbootState,
    [cancel, message, phase, reset, sessionId, start],
  );

  return (
    <ForceFastbootContext.Provider value={value}>
      {children}
    </ForceFastbootContext.Provider>
  );
}

export function useForceFastboot() {
  const context = useContext(ForceFastbootContext);
  if (!context) throw new Error("useForceFastboot must be used within ForceFastbootProvider");
  return context;
}
