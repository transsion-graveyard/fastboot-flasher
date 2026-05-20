import { useMemo, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpen, Search, Send, TerminalSquare } from "lucide-react";
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

interface ExtraToolsProps {
  disabled?: boolean;
  flashing?: boolean;
  onGetVariable: (name: string) => Promise<string>;
  onGetAllVariables: () => Promise<Record<string, string>>;
  onManualFlash: (
    partition: string,
    imagePath: string,
    slot: "" | "a" | "b" | "active" | "inactive" | "all",
  ) => Promise<void>;
}

export function ExtraTools({
  disabled = false,
  flashing = false,
  onGetVariable,
  onGetAllVariables,
  onManualFlash,
}: ExtraToolsProps) {
  const [variableName, setVariableName] = useState("");
  const [variableOutput, setVariableOutput] = useState("");
  const [manualPartition, setManualPartition] = useState("");
  const [manualSlot, setManualSlot] = useState<"" | "a" | "b" | "active" | "inactive" | "all">("");
  const [manualImage, setManualImage] = useState("");
  const [reading, setReading] = useState(false);
  const [pickingImage, setPickingImage] = useState(false);
  const { append } = useFlashLog();

  const manualDisabled = disabled || flashing || !manualPartition.trim() || !manualImage;
  const prettyVars = useMemo(() => variableOutput, [variableOutput]);

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

  const readVariable = async () => {
    const trimmed = variableName.trim();
    if (!trimmed) {
      toast.error("Enter a fastboot variable name");
      return;
    }

    setReading(true);
    append(`Getvar Started ${trimmed}`);
    try {
      const value = await onGetVariable(trimmed);
      setVariableOutput(value);
      append(`Getvar Complete ${trimmed}`);
    } catch (error) {
      append(`Getvar Error ${trimmed} ${error}`);
      toast.error(String(error));
    } finally {
      setReading(false);
    }
  };

  const readAllVariables = async () => {
    setReading(true);
    append("GetvarAll Started");
    try {
      const vars = await onGetAllVariables();
      setVariableOutput(JSON.stringify(vars, null, 2));
      append("GetvarAll Complete");
    } catch (error) {
      append(`GetvarAll Error ${error}`);
      toast.error(String(error));
    } finally {
      setReading(false);
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
    <div className="grid gap-5 xl:grid-cols-[minmax(0,0.95fr)_minmax(0,1.05fr)]">
      <SectionCard title="Fastboot vars" contentClassName="space-y-4">
        <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_auto]">
          <Input
            value={variableName}
            onChange={(event) => setVariableName(event.target.value)}
            placeholder="e.g. current-slot"
            aria-label="Fastboot variable"
            disabled={disabled || reading}
          />
          <Button
            variant="outline"
            className="gap-2"
            disabled={disabled || reading}
            onClick={readVariable}
          >
            <Search className="h-4 w-4" />
            {reading ? "Reading..." : "Read var"}
          </Button>
        </div>
        <Button
          variant="outline"
          className="w-full justify-start gap-2"
          disabled={disabled || reading}
          onClick={readAllVariables}
        >
          <TerminalSquare className="h-4 w-4" />
          Read all vars
        </Button>
        <pre className="min-h-48 overflow-auto rounded-md border border-border/70 bg-muted/20 p-3 text-xs leading-5 text-muted-foreground">
          {prettyVars || "Variable output will appear here."}
        </pre>
      </SectionCard>

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
    </div>
  );
}
