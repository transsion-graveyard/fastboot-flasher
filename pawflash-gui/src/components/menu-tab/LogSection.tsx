import { memo, useEffect, useRef } from "react";
import { Copy, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Button } from "@/components/ui/button";
import { useFlashLog } from "@/hooks/useFlashProgress";
import { SectionCard } from "@/components/menu-tab/SectionCard";

function logLevel(entry: string): "info" | "success" | "warning" | "error" {
  if (
    entry.startsWith("PartitionComplete") ||
    entry.startsWith("EraseComplete") ||
    entry.startsWith("Complete") ||
    entry.startsWith("PlanBuilt")
  ) {
    return "success";
  }
  if (entry.startsWith("PartitionFailed") || entry.startsWith("Error")) {
    return "error";
  }
  if (entry.startsWith("DeviceProbeError")) {
    return "error";
  }
  if (entry.startsWith("DeviceProbeWarning")) {
    return "warning";
  }
  if (entry.startsWith("PartitionSkipped")) {
    return "warning";
  }
  return "info";
}

const logColors: Record<string, string> = {
  info: "text-muted-foreground",
  success: "text-success",
  warning: "text-warning",
  error: "text-error",
};

export const LogSection = memo(function LogSection() {
  const { entries, clear } = useFlashLog();
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (entries.length > 0) {
      bottomRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [entries.length]);

  const copyEntries = async () => {
    try {
      await navigator.clipboard.writeText(entries.join("\n"));
      toast.success("Log copied");
    } catch (error) {
      toast.error(String(error));
    }
  };

  return (
    <SectionCard
      title="Log"
      className="flex min-h-0 flex-1 flex-col"
      contentClassName="flex min-h-0 flex-1 flex-col"
      headerActions={
        <>
          <Button
            variant="outline"
            size="sm"
            onClick={clear}
            disabled={entries.length === 0}
          >
            <Trash2 className="h-4 w-4" />
            Clear Log
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={copyEntries}
            disabled={entries.length === 0}
          >
            <Copy className="h-4 w-4" />
            Copy Log
          </Button>
        </>
      }
    >
      <ScrollArea className="panel-inset min-h-0 flex-1">
        <div className="space-y-1.5 p-4 font-mono text-sm" aria-live="polite" aria-atomic="false">
          {entries.length === 0 ? (
            <div className="max-w-[48ch] whitespace-pre-wrap text-muted-foreground italic">
              Run an action to populate the log.
            </div>
          ) : (
            entries.map((entry, index) => (
              <div key={`${index}-${entry}`} className={logColors[logLevel(entry)]}>
                {entry}
              </div>
            ))
          )}
          <div ref={bottomRef} />
        </div>
      </ScrollArea>
    </SectionCard>
  );
});
