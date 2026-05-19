import { type ReactNode, useCallback, useEffect, useMemo, useState } from "react";
import { Layers3, Moon, PanelLeftClose, PanelLeftOpen, Settings2, Sun, Zap } from "lucide-react";
import { Separator } from "@/components/ui/separator";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useFlashLog } from "@/hooks/useFlashProgress";

type AppTheme = "light" | "dark";

interface AppLayoutProps {
  children: (props: { tab: "main" | "extra" | "menu" }) => ReactNode;
  sidebarStatus?: ReactNode;
  sidebarActions?: ReactNode;
  theme: AppTheme;
  onThemeChange: (theme: AppTheme | ((current: AppTheme) => AppTheme)) => void;
}

const themeOptions: Array<{
  value: AppTheme;
  label: string;
  icon: typeof Sun;
}> = [
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
];

export function AppLayout({
  children,
  sidebarStatus,
  sidebarActions,
  theme,
  onThemeChange,
}: AppLayoutProps) {
  const [tab, setTab] = useState<"main" | "extra" | "menu">("main");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const { append } = useFlashLog();

  const handleTabChange = useCallback(
    (newTab: "main" | "extra" | "menu") => {
      append(`TabSwitch ${newTab}`);
      setTab(newTab);
    },
    [append],
  );

  const handleSidebarToggle = useCallback(() => {
    setSidebarOpen((prev) => {
      const next = !prev;
      append(`SidebarToggle ${next ? "open" : "closed"}`);
      return next;
    });
  }, [append]);

  useEffect(() => {
    if (typeof window === "undefined") {
      return;
    }

    window.localStorage.setItem("app-theme", theme);

    const root = document.documentElement;
    root.classList.toggle("dark", theme === "dark");
  }, [theme]);

  const navItems = useMemo(
    () => [
      { key: "main" as const, label: "Flasher", icon: Zap },
      { key: "menu" as const, label: "Menu", icon: Settings2 },
      { key: "extra" as const, label: "Extra", icon: Layers3 },
    ],
    [],
  );

  return (
    <div className="flex h-screen bg-background text-foreground">
      <aside
        aria-label="Sidebar"
        className={cn(
          "flex shrink-0 flex-col border-r border-sidebar-border bg-sidebar transition-[width] duration-200 ease-out",
          sidebarOpen ? "w-56" : "w-[4.5rem]",
        )}
      >
        <div className={cn("flex items-center", sidebarOpen ? "justify-between px-3 py-3" : "justify-center px-2 py-3")}>
          {sidebarOpen && <span className="text-sm font-semibold tracking-[0.16em] text-muted-foreground">PAWFLASH</span>}
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={sidebarOpen ? "Collapse sidebar" : "Expand sidebar"}
            onClick={handleSidebarToggle}
          >
            {sidebarOpen ? <PanelLeftClose className="h-4 w-4" /> : <PanelLeftOpen className="h-4 w-4" />}
          </Button>
        </div>
        <Separator />
        <nav aria-label="Main navigation" className={cn("flex flex-col gap-2 p-3", !sidebarOpen && "items-center px-2")}>
          {navItems.map((item) => {
            const Icon = item.icon;
            const active = tab === item.key;
            return (
              <button
                key={item.key}
                onClick={() => handleTabChange(item.key)}
                aria-label={item.label}
                className={cn(
                  "flex items-center gap-3 rounded-md px-3 py-2.5 text-sm font-medium transition-[background-color,color,box-shadow] duration-200 ease-out",
                  sidebarOpen ? "w-full justify-start" : "w-11 justify-center px-0",
                    active
                      ? "bg-accent-brand/12 text-accent-soft-foreground shadow-[var(--panel-shadow)] border border-accent-brand/25"
                      : "text-muted-foreground hover:bg-accent-soft/70 hover:text-foreground",
                )}
              >
                <Icon className="h-4 w-4 shrink-0" />
                {sidebarOpen && <span className="truncate">{item.label}</span>}
              </button>
            );
          })}
        </nav>
        <div className="min-h-0 flex-1" />
        {sidebarStatus && (
          <div className={cn("space-y-3 p-3", !sidebarOpen && "px-2")}>{sidebarStatus}</div>
        )}
        {sidebarActions && (
          <div className={cn("p-3", !sidebarOpen && "px-2")}>{sidebarActions}</div>
        )}
        <Separator />
        <div className={cn("p-3", !sidebarOpen && "px-2")}>
          {sidebarOpen ? (
            <div className="grid grid-cols-2 gap-2">
              {themeOptions.map((option) => {
                const Icon = option.icon;
                return (
                  <Button
                    key={option.value}
                    variant={theme === option.value ? "secondary" : "ghost"}
                    size="icon-sm"
                    className={cn(
                      "w-full",
                      theme === option.value &&
                        theme === "light" &&
                        "border border-sidebar-border/70 bg-sidebar-accent text-sidebar-accent-foreground shadow-[var(--panel-shadow)] hover:bg-sidebar-accent/90",
                    )}
                    aria-label={`Theme ${option.label}`}
                    title={option.label}
                    onClick={() => {
                      append(`ThemeChanged ${option.value}`);
                      onThemeChange(option.value);
                    }}
                  >
                    <Icon className="h-4 w-4" />
                  </Button>
                );
              })}
            </div>
          ) : (
            <Button
              variant="ghost"
              size="icon-sm"
              className="w-full"
              aria-label={`Theme: ${theme}`}
              onClick={() => {
                append("ThemeToggle");
                onThemeChange((current) => (current === "light" ? "dark" : "light"));
              }}
            >
              {theme === "light" ? <Sun className="h-4 w-4" /> : <Moon className="h-4 w-4" />}
            </Button>
          )}
        </div>
      </aside>

      <main className="flex min-w-0 flex-1 overflow-hidden">
        <div className="flex min-h-0 flex-1 flex-col p-4 xl:p-5">{children({ tab })}</div>
      </main>
    </div>
  );
}
