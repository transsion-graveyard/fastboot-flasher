import { memo } from "react";
import { FilePenLine } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Checkbox } from "@/components/ui/checkbox";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Button } from "@/components/ui/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import type { PartitionDto } from "@/types/api";

interface PartitionTableProps {
  partitions: PartitionDto[];
  isParsingPlan?: boolean;
  onToggle: (index: number) => void;
  onToggleAll: () => void;
  allSelected: boolean;
  someSelected: boolean;
  onPickImage: (partition: PartitionDto) => void;
  className?: string;
}

const safetyColor: Record<
  string,
  "default" | "destructive" | "success" | "warning" | "secondary" | "outline"
> = {
  dangerous: "destructive",
  identity_or_calibration: "destructive",
  bootloader_critical: "success",
  preloader: "success",
  boot_critical: "warning",
  firmware: "secondary",
  android_system: "secondary",
  wipe_only: "outline",
  regional: "outline",
  other: "outline",
};

export const PartitionTable = memo(function PartitionTable({
  partitions,
  isParsingPlan = false,
  onToggle,
  onToggleAll,
  allSelected,
  someSelected,
  onPickImage,
  className,
}: PartitionTableProps) {
  if (partitions.length === 0) {
    return (
      <div
        className={cn(
          "panel-shell flex min-h-0 flex-1 items-center justify-center p-8 text-center",
          className,
        )}
      >
        <div className="max-w-[40ch] space-y-2">
          <p className="text-base font-medium text-foreground">
            {isParsingPlan ? "Refreshing flash plan" : "No flash plan loaded"}
          </p>
          <p className="text-sm leading-6 text-muted-foreground">
            {isParsingPlan
              ? "Reviewing the selected firmware source and rebuilding the partition list."
              : "Select a scatter file or firmware manifest to review partitions and prepare the flash set."}
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className={cn("panel-shell flex min-h-0 flex-1 flex-col", className)}>
      <ScrollArea className="min-h-0 flex-1">
        <Table className="table-fixed">
          <TableHeader className="[&_th]:sticky [&_th]:top-0 [&_th]:z-10 [&_th]:border-b [&_th]:border-border/80 [&_th]:bg-card/96 [&_th]:text-muted-foreground">
            <TableRow>
              <TableHead className="w-12">
                <Checkbox
                  checked={allSelected}
                  indeterminate={someSelected}
                  onCheckedChange={onToggleAll}
                  aria-label={allSelected ? "Clear all partitions" : "Select all partitions"}
                />
              </TableHead>
              <TableHead className="w-36">Partition</TableHead>
              <TableHead className="w-28">Size</TableHead>
              <TableHead className="w-40">Type</TableHead>
              <TableHead className="w-56">Image</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {partitions.map((partition) => (
              <TableRow key={partition.index}>
                <TableCell>
                  <Checkbox
                    checked={partition.selected}
                    onCheckedChange={() => onToggle(partition.index)}
                    aria-label={`Select ${partition.partition}`}
                  />
                </TableCell>
                <TableCell className="truncate font-mono" title={partition.partition}>
                  {partition.partition}
                </TableCell>
                <TableCell className="whitespace-nowrap">{partition.size_human}</TableCell>
                <TableCell className="truncate">
                  <Badge variant={safetyColor[partition.safety_class] ?? "outline"}>
                    {partition.safety_class}
                  </Badge>
                </TableCell>
                <TableCell>
                  {partition.action === "flash" ? (
                    <Tooltip>
                      <TooltipTrigger render={
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className={cn(
                            "h-auto w-full justify-start gap-2 px-0 py-1 text-left hover:bg-transparent",
                            partition.image_overridden && "text-accent-brand",
                          )}
                          onClick={() => onPickImage(partition)}
                        >
                          <FilePenLine className="h-4 w-4" />
                          <span className="truncate">
                            {partition.image_name ?? "Choose image"}
                          </span>
                        </Button>
                      }>
                      </TooltipTrigger>
                      <TooltipContent side="top" align="start" className="max-w-sm break-all">
                        {partition.image_path ?? "No image resolved"}
                      </TooltipContent>
                    </Tooltip>
                  ) : (
                    <span className="text-muted-foreground">—</span>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </ScrollArea>
    </div>
  );
});
