import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "sonner";
import { ShieldOff, Zap } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/shared/ConfirmDialog";
import { useFlashLog, useFlashProgress } from "@/hooks/useFlashProgress";
import { SectionCard } from "@/components/menu-tab/SectionCard";

interface DeviceSectionProps {
  onForceFastboot: () => void | Promise<void>;
  forceFastbootDisabled?: boolean;
  disableVbmetaDisabled?: boolean;
  disabled?: boolean;
}

export function DeviceSection({
  onForceFastboot,
  forceFastbootDisabled = false,
  disableVbmetaDisabled = false,
  disabled = false,
}: DeviceSectionProps) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [aborting, setAborting] = useState(false);
  const { reset } = useFlashProgress();
  const { append } = useFlashLog();

  return (
    <SectionCard
      title="Device"
      contentClassName="space-y-3"
    >
        <Button
          className="w-full justify-start gap-3"
          disabled={disabled || busy || forceFastbootDisabled}
          onClick={onForceFastboot}
        >
          <Zap className="h-4 w-4" />
          Force reboot fastboot
        </Button>
        <Button
          variant="outline"
          className="w-full justify-start gap-3"
          disabled={busy || disableVbmetaDisabled}
          onClick={() => setOpen(true)}
        >
          <ShieldOff className="h-4 w-4" />
          Disable Vbmeta
        </Button>
      
      <ConfirmDialog
        open={open}
        onOpenChange={(nextOpen) => {
          if (!nextOpen && busy && !aborting) {
            setAborting(true);
            invoke("cancel_flash").catch(() => {}).finally(() => setAborting(false));
            return;
          }
          setOpen(nextOpen);
        }}
        title="Disable Vbmeta"
        destructive
        confirmLabel="Disable"
        isPending={busy || aborting}
        onConfirm={async () => {
          setBusy(true);
          try {
            append("DisableVbmeta Started");
            await invoke("disable_vbmeta");
            reset();
            setOpen(false);
            append("DisableVbmeta Complete");
            toast.success("Vbmeta disabled");
          } catch (error) {
            const message = error instanceof Error ? error.message : String(error);
            if (message.toLocaleLowerCase().includes("cancelled")) {
              append("DisableVbmeta Cancelled");
            } else {
              append(`DisableVbmeta Error ${message}`);
              toast.error(message);
            }
            setOpen(false);
          } finally {
            setBusy(false);
          }
        }}
      />
    </SectionCard>
  );
}
