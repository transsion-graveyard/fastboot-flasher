import { useState } from "react";
import { Eraser } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/shared/ConfirmDialog";
import { SectionCard } from "@/components/menu-tab/SectionCard";

interface DataSectionProps {
  onWipeData: () => void | Promise<void>;
  disabled?: boolean;
}

export function DataSection({ onWipeData, disabled = false }: DataSectionProps) {
  const [open, setOpen] = useState(false);

  return (
    <SectionCard title="Data" contentClassName="space-y-3">
      <Button
        variant="destructive"
        className="w-full justify-start gap-3"
        disabled={disabled}
        onClick={() => setOpen(true)}
      >
        <Eraser className="h-4 w-4" />
        Wipe Data
      </Button>

      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title="Wipe Data"
        confirmLabel="Wipe Data"
        destructive
        onConfirm={async () => {
          setOpen(false);
          await onWipeData();
        }}
      />
    </SectionCard>
  );
}
