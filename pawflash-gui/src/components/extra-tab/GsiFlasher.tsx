import { memo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, Zap } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SectionCard } from "@/components/menu-tab/SectionCard";
import { useFlashLog } from "@/hooks/useFlashProgress";

interface GsiFlasherProps {
  imagePath: string;
  onImagePathChange: (path: string) => void;
  onFlash: () => void | Promise<void>;
  disabled?: boolean;
  flashing?: boolean;
}

export const GsiFlasher = memo(function GsiFlasher({
  imagePath,
  onImagePathChange,
  onFlash,
  disabled = false,
  flashing = false,
}: GsiFlasherProps) {
  const [picking, setPicking] = useState(false);
  const { append } = useFlashLog();

  const pickImage = async () => {
    setPicking(true);
    try {
      const selected = await open({
        title: "Select GSI system image",
        filters: [
          {
            name: "Android images",
            extensions: ["img"],
          },
        ],
        multiple: false,
      });

      if (typeof selected === "string") {
        const name = selected.split(/[/\\]/).pop() || selected;
        append(`GsiImagePicked ${name}`);
        onImagePathChange(selected);
        toast.success(`Loaded GSI image: ${name}`);
      }
    } finally {
      setPicking(false);
    }
  };

  return (
    <SectionCard
      title="GSI flasher"
      className="shrink-0"
      contentClassName="flex flex-col gap-4"
    >
      <div className="grid gap-3">
        <div className="grid grid-cols-2 gap-3">
          <Button
            variant="outline"
            className="gap-2"
            onClick={pickImage}
            disabled={disabled || picking || flashing}
          >
            <FolderOpen className="h-4 w-4" />
            {picking ? "Opening picker..." : "Select system image"}
          </Button>
          <Button
            className="h-10 min-w-44 gap-3 px-7"
            disabled={disabled || flashing || !imagePath}
            onClick={onFlash}
          >
            <Zap className="h-5 w-5" />
            {flashing ? "Starting..." : "Flash GSI"}
          </Button>
        </div>
        <Input
          value={imagePath}
          readOnly
          placeholder="No GSI image selected"
          aria-label="Selected GSI image path"
          title={imagePath || "No GSI image selected"}
        />
      </div>
    </SectionCard>
  );
});
