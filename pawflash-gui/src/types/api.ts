export interface PartitionDto {
  index: number;
  action: string;
  partition: string;
  size_human: string;
  size_bytes: number;
  safety_class: string;
  image_type: string | null;
  source: string;
  image_path: string | null;
  image_name: string | null;
  image_overridden?: boolean;
  selected: boolean;
}

export interface FlashSummaryDto {
  flash_count: number;
  wipe_count: number;
  skipped_count: number;
  total_bytes: number;
}

export interface FlashPlanDto {
  mode: string;
  storage: string;
  slot_policy: string;
  chipset: string | null;
  summary: FlashSummaryDto;
  partitions: PartitionDto[];
  warnings: string[];
  errors: string[];
}

export interface ParseScatterResponseDto {
  plan_id: number;
  plan: FlashPlanDto;
}

export interface ForceFastbootStartDto {
  session_id: number;
}

export interface DeviceInfo {
  serial: string;
  product: string;
  slot: string;
  secure: string;
  unlocked: string;
  version: string;
  mode: string;
  all_vars: Record<string, string>;
}

export type FlashEvent =
  | { event: "WaitingForDevice" }
  | { event: "DeviceCheckDiagnostic"; data: { stage: string; level: string; message: string } }
  | { event: "GsiStatus"; data: { status: string } }
  | { event: "PlanBuilt"; data: { actions: number; total_bytes: number } }
  | { event: "PreparingImage"; data: { partition: string } }
  | { event: "Flashing"; data: { partition: string; bytes: number; total: number; speed_bps: number } }
  | { event: "Simulating"; data: { partition: string; action: string; bytes: number; total: number; speed_bps: number } }
  | { event: "PartitionComplete"; data: { partition: string } }
  | { event: "PartitionSkipped"; data: { partition: string; reason: string } }
  | { event: "PartitionFailed"; data: { partition: string; error: string } }
  | { event: "Erasing"; data: { partition: string } }
  | { event: "EraseComplete"; data: { partition: string } }
  | { event: "Overall"; data: { bytes: number; total: number } }
  | { event: "Complete"; data: { summary: FlashSummaryDto } }
  | { event: "Cancelled"; data: { message: string } }
  | { event: "Error"; data: { message: string } };

export type ForceFastbootEvent =
  | { event: "Started"; data: { session_id: number } }
  | { event: "WaitingForPreloader"; data: { session_id: number } }
  | { event: "Complete"; data: { session_id: number } }
  | { event: "Cancelled"; data: { session_id: number } }
  | { event: "Error"; data: { session_id: number; message: string } };
