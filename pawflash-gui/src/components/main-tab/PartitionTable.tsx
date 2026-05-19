import { memo } from "react";
import { FilePenLine } from "lucide-react";
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

function partitionImageLabel(partition: PartitionDto) {
  return partition.image_name ?? partition.image_path ?? "—";
}

function partitionImageHint(partition: PartitionDto) {
  return partition.image_path ?? partition.image_name ?? "No image resolved";
}

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
  const columnWidths = ["w-12", "w-36", "w-28", "w-40", "w-56"];

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
      <div className="border-b border-border/80 bg-card/96">
        <Table className="table-fixed">
          <colgroup>
            {columnWidths.map((width) => (
              <col key={width} className={width} />
            ))}
          </colgroup>
          <TableHeader className="[&_th]:text-muted-foreground">
            <TableRow>
              <TableHead className={cn(columnWidths[0], "px-0 text-center")}>
                <div className="flex justify-center">
                  <Checkbox
                    checked={allSelected}
                    indeterminate={someSelected}
                    onCheckedChange={onToggleAll}
                    aria-label={allSelected ? "Clear all partitions" : "Select all partitions"}
                  />
                </div>
              </TableHead>
              <TableHead className={columnWidths[1]}>Partition</TableHead>
              <TableHead className={columnWidths[2]}>Size</TableHead>
              <TableHead className={columnWidths[3]}>Type</TableHead>
              <TableHead className={columnWidths[4]}>Image</TableHead>
            </TableRow>
          </TableHeader>
        </Table>
      </div>
      <ScrollArea className="min-h-0 flex-1">
        <Table className="table-fixed">
          <colgroup>
            {columnWidths.map((width) => (
              <col key={width} className={width} />
            ))}
          </colgroup>
          <TableBody>
            {partitions.map((partition) => (
              <TableRow key={partition.index}>
                <TableCell className="px-0 text-center">
                  <div className="flex justify-center">
                    <Checkbox
                      checked={partition.selected}
                      onCheckedChange={() => onToggle(partition.index)}
                      aria-label={`Select ${partition.partition}`}
                    />
                  </div>
                </TableCell>
                <TableCell className="truncate font-mono text-left" title={partition.partition}>
                  {partition.partition}
                </TableCell>
                <TableCell className="whitespace-nowrap text-center">{partition.size_human}</TableCell>
                <TableCell className="truncate text-center">
                  {partition.image_type ? partition.image_type : <span className="text-muted-foreground">—</span>}
                </TableCell>
                <TableCell className="text-left">
                  {partition.action === "flash" ? (
                    <Tooltip>
                      <TooltipTrigger
                        render={
                          <Button
                            type="button"
                            variant="ghost"
                            size="sm"
                            className={cn(
                              "h-auto w-full min-w-0 justify-start gap-2 px-0 py-1 text-left hover:bg-transparent",
                              partition.image_overridden && "text-accent-brand",
                            )}
                            onClick={() => onPickImage(partition)}
                          >
                            <FilePenLine className="h-4 w-4" />
                            <span className="min-w-0 truncate">
                              {partition.image_name ?? "Choose image"}
                            </span>
                          </Button>
                        }
                      >
                      </TooltipTrigger>
                      <TooltipContent side="top" align="start" className="max-w-sm break-all">
                        {partitionImageHint(partition)}
                      </TooltipContent>
                    </Tooltip>
                  ) : (
                    <span
                      className={cn(
                        "block min-w-0 truncate",
                        partitionImageLabel(partition) === "—"
                          ? "text-muted-foreground"
                          : "font-mono",
                      )}
                      title={partitionImageHint(partition)}
                    >
                      {partitionImageLabel(partition)}
                    </span>
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
