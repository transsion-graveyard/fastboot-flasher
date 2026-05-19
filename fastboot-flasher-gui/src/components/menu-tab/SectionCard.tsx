import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

interface SectionCardProps {
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
}: SectionCardProps) {
  return (
    <section className={cn("panel-shell p-4 md:p-5", className)}>
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
