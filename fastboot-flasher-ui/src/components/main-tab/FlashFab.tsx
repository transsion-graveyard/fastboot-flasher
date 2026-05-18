import { memo } from "react";
import { Button } from "@/components/ui/button";
import { Zap } from "lucide-react";

interface FlashFabProps {
  onClick: () => void;
  disabled?: boolean;
}

export const FlashFab = memo(function FlashFab({ onClick, disabled }: FlashFabProps) {
  return (
    <div className="flex justify-end">
      <Button
        size="lg"
        onClick={onClick}
        disabled={disabled}
        className="min-w-44 gap-3 px-7"
      >
        <Zap className="h-5 w-5" aria-hidden="true" />
        Start Flash
      </Button>
    </div>
  );
});
