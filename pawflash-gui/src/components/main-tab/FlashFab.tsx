import { memo } from "react";
import { Button } from "@/components/ui/button";
import { Zap } from "lucide-react";

interface FlashFabProps {
  onClick: () => void;
  disabled?: boolean;
}

export const FlashFab = memo(function FlashFab({ onClick, disabled }: FlashFabProps) {
  return (
    <Button
      size="lg"
      onClick={onClick}
      disabled={disabled}
      className="w-full gap-1.5 px-2.5 text-sm lg:gap-3 lg:px-7 lg:text-base"
    >
      <Zap className="h-5 w-5" aria-hidden="true" />
      Start Flash
    </Button>
  );
});
