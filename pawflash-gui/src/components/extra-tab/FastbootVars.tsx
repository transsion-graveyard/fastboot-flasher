import { memo, useState } from "react";
import { Copy, Search, TerminalSquare } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { SectionCard } from "@/components/menu-tab/SectionCard";
import { useFlashLog } from "@/hooks/useFlashProgress";

interface FastbootVarsProps {
  disabled?: boolean;
  onGetVariable: (name: string) => Promise<string>;
  onGetAllVariables: () => Promise<Record<string, string>>;
}

export const FastbootVars = memo(function FastbootVars({
  disabled = false,
  onGetVariable,
  onGetAllVariables,
}: FastbootVarsProps) {
  const [variableName, setVariableName] = useState("");
  const [variableOutput, setVariableOutput] = useState("");
  const [reading, setReading] = useState(false);
  const { append } = useFlashLog();

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

  return (
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
      <div className="grid grid-cols-2 gap-3">
        <Button
          variant="outline"
          className="justify-start gap-2"
          disabled={disabled || reading}
          onClick={readAllVariables}
        >
          <TerminalSquare className="h-4 w-4" />
          Read all vars
        </Button>
        <Button
          variant="outline"
          className="justify-start gap-2"
          disabled={disabled || reading || !variableOutput}
          onClick={() => {
            navigator.clipboard.writeText(variableOutput);
            toast.success("Copied to clipboard");
          }}
        >
          <Copy className="h-4 w-4" />
          Copy vars
        </Button>
      </div>
      <pre className="min-h-48 overflow-auto rounded-md border border-border/70 bg-muted/20 p-3 text-xs leading-5 text-muted-foreground">
        {variableOutput || "Variable output will appear here."}
      </pre>
    </SectionCard>
  );
});
