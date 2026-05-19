import { useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { DeviceInfo } from "@/types/api";

export function useDevice() {
  const connect = useCallback(() => invoke<DeviceInfo>("connect_device"), []);
  const check = useCallback(() => invoke<DeviceInfo>("check_device"), []);
  const reboot = useCallback(() => invoke<void>("reboot_device"), []);
  const rebootBootloader = useCallback(
    () => invoke<void>("reboot_bootloader"),
    [],
  );
  const rebootFastboot = useCallback(
    () => invoke<void>("reboot_fastboot"),
    [],
  );
  const rebootRecovery = useCallback(
    () => invoke<void>("reboot_recovery"),
    [],
  );
  const forceFastboot = useCallback(
    () => invoke<void>("force_fastboot_cmd"),
    [],
  );
  const setActiveSlot = useCallback(
    (slot: "a" | "b") => invoke<void>("set_active_slot", { slot }),
    [],
  );
  const unlockBootloader = useCallback(
    () => invoke<void>("unlock_bootloader"),
    [],
  );
  const lockBootloader = useCallback(
    () => invoke<void>("lock_bootloader"),
    [],
  );

  return {
    check,
    connect,
    reboot,
    rebootBootloader,
    rebootFastboot,
    rebootRecovery,
    forceFastboot,
    setActiveSlot,
    unlockBootloader,
    lockBootloader,
  };
}
