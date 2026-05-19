import { memo, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen } from "lucide-react";
import { toast } from "sonner";
import { useFlashLog } from "@/hooks/useFlashProgress";

interface ScatterPickerProps {
  path: string;
  onChange: (path: string) => void;
}

export const ScatterPicker = memo(function ScatterPicker({ path, onChange }: ScatterPickerProps) {
  const [picking, setPicking] = useState(false);
  const { append } = useFlashLog();

  const pick = async () => {
    setPicking(true);
    try {
      const selected = await open({
        title: "Select scatter or firmware manifest",
        filters: [
          {
            name: "Scatter files",
            extensions: ["xml", "txt"],
          },
        ],
        multiple: false,
      });
      if (typeof selected === "string") {
        const name = selected.split(/[/\\]/).pop() || selected;
        append(`ScatterPicked ${name}`);
        onChange(selected);
        toast.success(`Scatter loaded: ${name}`);
      }
    } finally {
      setPicking(false);
    }
  };

  return (
    <section className="panel-shell p-4">
      <div className="grid min-w-0 gap-3 sm:grid-cols-[auto_minmax(0,1fr)]">
        <Button variant="outline" onClick={pick} disabled={picking} className="gap-2">
          <FolderOpen className="h-4 w-4" />
          {picking ? "Opening picker..." : "Select manifest"}
        </Button>
        <Input
          value={path}
          readOnly
          placeholder="No scatter or manifest selected"
          className="min-w-0 flex-1"
          aria-label="Selected scatter or firmware manifest path"
          title={path || "No scatter or manifest selected"}
        />
      </div>
    </section>
  );
});
