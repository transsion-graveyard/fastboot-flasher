import { memo, useState } from "react";
import { RotateCcw } from "lucide-react";
import { toast } from "sonner";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { flashModeLabel, visibleFlashModeOptions } from "@/lib/flash-mode";
import { useDevice } from "@/hooks/useDevice";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";

interface FlashOptionsProps {
  mode: string;
  onModeChange: (mode: string) => void;
  reboot: boolean;
  onRebootChange: (v: boolean) => void;
  advanced: boolean;
  onAdvancedChange: (v: boolean) => void;
  includePreloader: boolean;
  onIncludePreloaderChange: (v: boolean) => void;
  slot: "" | "a" | "b" | "all";
  onSlotChange: (slot: "" | "a" | "b" | "all") => void;
}

export const FlashOptions = memo(function FlashOptions({
  mode,
  onModeChange,
  reboot,
  onRebootChange,
  advanced,
  onAdvancedChange,
  includePreloader,
  onIncludePreloaderChange,
  slot,
  onSlotChange,
}: FlashOptionsProps) {
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [rebooting, setRebooting] = useState(false);
  const advancedEnabled = includePreloader || slot !== "";
  const modeOptions = visibleFlashModeOptions();
  const slotLabel = slot === "a" ? "_a" : slot === "b" ? "_b" : slot === "all" ? "all slots" : "";
  const { reboot: rebootDevice } = useDevice();

  const handleReboot = async () => {
    setRebooting(true);
    try {
      await rebootDevice();
      toast.success("Rebooted to system");
    } catch (error) {
      toast.error(String(error));
    } finally {
      setRebooting(false);
    }
  };

  return (
    <div className="panel-shell grid gap-4 p-4 xl:grid-cols-[minmax(0,1fr)_auto]">
      <div className="grid gap-3 lg:grid-cols-[minmax(0,18rem)_minmax(0,1fr)]">
        <Select value={mode} onValueChange={(v) => v !== null && onModeChange(v)}>
          <SelectTrigger aria-label="Flash mode">
            <SelectValue>{flashModeLabel(mode)}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            {modeOptions.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>

        <div className="flex items-center gap-3 px-1 py-1">
          <Checkbox
            id="reboot"
            checked={reboot}
            onCheckedChange={(v) => onRebootChange(!!v)}
          />
          <Label htmlFor="reboot">Reboot after flash</Label>
        </div>
      </div>

      <div className="flex items-start gap-2 justify-start xl:justify-end">
        <Button
          type="button"
          variant="outline"
          className="gap-2"
          disabled={rebooting}
          onClick={handleReboot}
        >
          <RotateCcw className="h-4 w-4" />
          {rebooting ? "Rebooting..." : "Reboot"}
        </Button>
        <Button
          type="button"
          variant={advanced ? "secondary" : "outline"}
          className={cn("gap-2", advancedEnabled && "animate-pulse")}
          onClick={() => {
            onAdvancedChange(true);
            setAdvancedOpen(true);
          }}
        >
          Advanced
        </Button>
        <Dialog open={advancedOpen} onOpenChange={setAdvancedOpen}>
          <DialogContent className="sm:max-w-md">
            <DialogHeader>
              <DialogTitle>Advanced plan filters</DialogTitle>
            </DialogHeader>

            <div className="space-y-4">
              <div className="flex items-center gap-3 px-1 py-1">
                <Checkbox
                  id="advanced-include-preloader"
                  checked={includePreloader}
                  onCheckedChange={(v) => {
                    onAdvancedChange(true);
                    onIncludePreloaderChange(!!v);
                  }}
                />
                <Label htmlFor="advanced-include-preloader">Include preloader</Label>
              </div>

              <div className="space-y-2">
                <Label htmlFor="advanced-slot">Slot override</Label>
                <Select
                  value={slot}
                  onValueChange={(value) => {
                    onAdvancedChange(true);
                    onSlotChange(value as "" | "a" | "b" | "all");
                  }}
                >
                  <SelectTrigger aria-label="Slot override">
                    <SelectValue placeholder="Use plan default">
                      {slotLabel || undefined}
                    </SelectValue>
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="a">_a</SelectItem>
                    <SelectItem value="b">_b</SelectItem>
                    <SelectItem value="all">all slots</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>

            <DialogFooter className="sm:justify-between">
              <Button
                type="button"
                variant="outline"
                onClick={() => {
                  onAdvancedChange(false);
                  onIncludePreloaderChange(false);
                  onSlotChange("");
                  setAdvancedOpen(false);
                }}
              >
                Reset
              </Button>
              <Button type="button" onClick={() => setAdvancedOpen(false)}>
                Done
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>
    </div>
  );
});
