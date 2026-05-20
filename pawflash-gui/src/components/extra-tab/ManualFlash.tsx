import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, Send } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { SectionCard } from "@/components/menu-tab/SectionCard";
import { useFlashLog } from "@/hooks/useFlashProgress";

interface ManualFlashProps {
  disabled?: boolean;
  flashing?: boolean;
  onManualFlash: (
    partition: string,
    imagePath: string,
    slot: "" | "a" | "b" | "active" | "inactive" | "all",
  ) => Promise<void>;
}

export function ManualFlash({
  disabled = false,
  flashing = false,
  onManualFlash,
}: ManualFlashProps) {
  const [manualPartition, setManualPartition] = useState("");
  const [manualSlot, setManualSlot] = useState<"" | "a" | "b" | "active" | "inactive" | "all">("");
  const [manualImage, setManualImage] = useState("");
  const [pickingImage, setPickingImage] = useState(false);
  const { append } = useFlashLog();

  const manualDisabled = disabled || flashing || !manualPartition.trim() || !manualImage;

  const pickManualImage = async () => {
    setPickingImage(true);
    try {
      const selected = await open({
        title: "Select image to flash",
        filters: [{ name: "Android images", extensions: ["img"] }],
        multiple: false,
      });
      if (typeof selected === "string") {
        setManualImage(selected);
        append(`ManualFlashImagePicked ${selected.split(/[/\\\\]/).pop() || selected}`);
      }
    } finally {
      setPickingImage(false);
    }
  };

  const startManualFlash = async () => {
    const partition = manualPartition.trim();
    if (!partition || !manualImage) {
      toast.error("Partition and image are required");
      return;
    }

    append(`ManualFlash Started partition=${partition} slot=${manualSlot || "default"}`);
    await onManualFlash(partition, manualImage, manualSlot);
  };

  return (
    <SectionCard title="Manual flash" contentClassName="space-y-4">
      <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_12rem]">
        <Input
          value={manualPartition}
          onChange={(event) => setManualPartition(event.target.value)}
          placeholder="partition name"
          aria-label="Manual flash partition"
          disabled={disabled || flashing}
        />
        <Select
          value={manualSlot}
          onValueChange={(value) =>
            setManualSlot(value as "" | "a" | "b" | "active" | "inactive" | "all")
          }
        >
          <SelectTrigger aria-label="Manual flash slot" disabled={disabled || flashing}>
            <SelectValue placeholder="Plan default" />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="a">_a</SelectItem>
            <SelectItem value="b">_b</SelectItem>
            <SelectItem value="active">active</SelectItem>
            <SelectItem value="inactive">inactive</SelectItem>
            <SelectItem value="all">all slots</SelectItem>
          </SelectContent>
        </Select>
      </div>
      <div className="grid gap-3 sm:grid-cols-[auto_minmax(0,1fr)]">
        <Button
          variant="outline"
          className="gap-2"
          disabled={disabled || flashing || pickingImage}
          onClick={pickManualImage}
        >
          <FolderOpen className="h-4 w-4" />
          {pickingImage ? "Opening..." : "Select image"}
        </Button>
        <Input
          value={manualImage}
          readOnly
          placeholder="No image selected"
          aria-label="Manual flash image path"
          disabled={disabled || flashing}
        />
      </div>
      <Button
        className="w-full justify-center gap-2"
        disabled={manualDisabled}
        onClick={startManualFlash}
      >
        <Send className="h-4 w-4" />
        {flashing ? "Starting..." : "Flash partition"}
      </Button>
    </SectionCard>
  );
}
