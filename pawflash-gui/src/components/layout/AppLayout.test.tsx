import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppLayout } from "./AppLayout";

const appendMock = vi.fn();

vi.mock("@/hooks/useFlashProgress", () => ({
  useFlashLog: () => ({
    append: appendMock,
  }),
}));

describe("AppLayout", () => {
  beforeEach(() => {
    Object.defineProperty(HTMLElement.prototype, "scrollTo", {
      value: vi.fn(),
      writable: true,
    });

    vi.stubGlobal("matchMedia", () => ({
      matches: false,
      media: "(max-width: 1100px)",
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    }));
  });

  afterEach(() => {
    vi.clearAllMocks();
    vi.unstubAllGlobals();
  });

  it("passes sidebar open state to sidebar actions", () => {
    render(
      <AppLayout
        theme="light"
        onThemeChange={() => undefined}
        sidebarActions={({ sidebarOpen }) => (
          <button type="button" aria-label="action">
            {sidebarOpen ? "open" : "closed"}
          </button>
        )}
      >
        {() => <div>content</div>}
      </AppLayout>,
    );

    expect(screen.getByRole("button", { name: "action" }).textContent).toBe("open");

    fireEvent.click(screen.getByRole("button", { name: "Collapse sidebar" }));

    expect(screen.getByRole("button", { name: "action" }).textContent).toBe("closed");
  });
});
