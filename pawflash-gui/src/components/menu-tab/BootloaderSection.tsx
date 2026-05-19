import { useState } from "react";
import { Lock, LockOpen } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useDevice } from "@/hooks/useDevice";
import { ConfirmDialog } from "@/components/shared/ConfirmDialog";
import { SectionCard } from "@/components/menu-tab/SectionCard";
import { toast } from "sonner";
import { useFlashLog } from "@/hooks/useFlashProgress";

export function BootloaderSection() {
  const { unlockBootloader, lockBootloader } = useDevice();
  const [unlockOpen, setUnlockOpen] = useState(false);
  const [lockOpen, setLockOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const { append } = useFlashLog();
  const disabled = busy;

  return (
    <SectionCard
      title="Bootloader"
      contentClassName="grid grid-cols-2 gap-3"
    >
        <Button
          variant="destructive"
          className="w-full justify-start gap-3"
          disabled={disabled}
          onClick={() => setUnlockOpen(true)}
        >
          <LockOpen className="h-4 w-4" />
          Unlock
        </Button>
        <Button
          variant="outline"
          className="w-full justify-start gap-3"
          disabled={disabled}
          onClick={() => setLockOpen(true)}
        >
          <Lock className="h-4 w-4" />
          Lock
        </Button>
      
      <ConfirmDialog
        open={unlockOpen}
        onOpenChange={setUnlockOpen}
        title="Unlock Bootloader"
        destructive
        confirmLabel="Unlock"
        isPending={busy}
        onConfirm={async () => {
          setBusy(true);
          append("BootloaderUnlock Started");
          try {
            await unlockBootloader();
            setUnlockOpen(false);
            append("BootloaderUnlock Complete");
            toast.success("Bootloader unlocked");
          } catch (e) {
            append(`BootloaderUnlock Error ${e}`);
            toast.error(String(e));
          } finally {
            setBusy(false);
          }
        }}
      />
      <ConfirmDialog
        open={lockOpen}
        onOpenChange={setLockOpen}
        title="Lock Bootloader"
        confirmLabel="Lock"
        isPending={busy}
        onConfirm={async () => {
          setBusy(true);
          append("BootloaderLock Started");
          try {
            await lockBootloader();
            setLockOpen(false);
            append("BootloaderLock Complete");
            toast.success("Bootloader locked");
          } catch (e) {
            append(`BootloaderLock Error ${e}`);
            toast.error(String(e));
          } finally {
            setBusy(false);
          }
        }}
      />
    </SectionCard>
  );
}
