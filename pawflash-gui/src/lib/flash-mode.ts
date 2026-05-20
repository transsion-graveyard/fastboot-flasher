export const FLASH_MODE_LABELS = {
  dry_run: "Dry run",
  dirty_flash: "Dirty flash",
  clean_flash: "Clean flash",
  selective: "Selective",
} as const;

export type FlashMode = keyof typeof FLASH_MODE_LABELS;

export const FLASH_MODE_OPTIONS: Array<{ value: FlashMode; label: string }> = [
  { value: "dry_run", label: FLASH_MODE_LABELS.dry_run },
  { value: "dirty_flash", label: FLASH_MODE_LABELS.dirty_flash },
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
  return "clean_flash";
}
