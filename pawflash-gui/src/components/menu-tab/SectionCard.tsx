import type { ReactNode } from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "@/lib/utils";

const sectionCardVariants = cva("", {
  variants: {
    variant: {
      default: "panel-shell p-4 md:p-5",
      flat: "rounded-md border border-border/60 bg-card/60 p-4 md:p-5",
    },
  },
  defaultVariants: {
    variant: "default",
  },
});

interface SectionCardProps extends VariantProps<typeof sectionCardVariants> {
  title: string;
  description?: string;
  headerActions?: ReactNode;
  children: ReactNode;
  className?: string;
  contentClassName?: string;
}

export function SectionCard({
  title,
  description,
  headerActions,
  children,
  className,
  contentClassName,
  variant,
}: SectionCardProps) {
  return (
    <section className={cn(sectionCardVariants({ variant }), className)}>
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0 space-y-1">
          <h3 className="text-sm font-semibold tracking-[0.04em] text-foreground">{title}</h3>
          {description ? (
            <p className="max-w-[48ch] text-sm text-muted-foreground">
              {description}
            </p>
          ) : null}
        </div>
        {headerActions ? <div className="flex shrink-0 items-center gap-2">{headerActions}</div> : null}
      </div>
      <div className={cn("mt-4", contentClassName)}>{children}</div>
    </section>
  );
}
