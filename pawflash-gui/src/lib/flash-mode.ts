export const FLASH_MODE_LABELS = {
  dry_run: "Dry run",
  firmware_upgrade: "Firmware upgrade",
  clean_flash: "Clean flash",
  selective: "Selective",
} as const;

export type FlashMode = keyof typeof FLASH_MODE_LABELS;

export const FLASH_MODE_OPTIONS: Array<{ value: FlashMode; label: string }> = [
  { value: "dry_run", label: FLASH_MODE_LABELS.dry_run },
  { value: "firmware_upgrade", label: FLASH_MODE_LABELS.firmware_upgrade },
  { value: "clean_flash", label: FLASH_MODE_LABELS.clean_flash },
  { value: "selective", label: FLASH_MODE_LABELS.selective },
];

export function flashModeLabel(mode: string) {
  return FLASH_MODE_LABELS[mode as FlashMode] ?? mode;
}

export function visibleFlashModeOptions() {
  return import.meta.env.DEV
    ? FLASH_MODE_OPTIONS
    : FLASH_MODE_OPTIONS.filter((option) => option.value !== "dry_run");
}

export function defaultFlashMode(): FlashMode {
  return import.meta.env.DEV ? "dry_run" : "firmware_upgrade";
}
