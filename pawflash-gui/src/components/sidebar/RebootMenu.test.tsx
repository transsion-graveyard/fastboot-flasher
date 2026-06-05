import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
const appendMock = vi.fn();
const successMock = vi.fn();
const errorMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: invokeMock,
}));

vi.mock("sonner", () => ({
  toast: {
    success: successMock,
    error: errorMock,
  },
}));

vi.mock("@/hooks/useFlashProgress", () => ({
  useFlashLog: () => ({
    append: appendMock,
  }),
}));

describe("RebootMenu", () => {
  afterEach(() => {
    vi.clearAllMocks();
  });

  it("reboots immediately to the selected target and closes the menu", async () => {
    const { RebootMenu } = await import("./RebootMenu");
    const onTargetChange = vi.fn();

    const { container } = render(
      <RebootMenu
        disabled={false}
        sidebarOpen
        target={null}
        onTargetChange={onTargetChange}
      />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Reboot" }));
    await waitFor(() => {
      expect(screen.getByRole("button", { name: "Reboot" }).getAttribute("data-popup-open")).not.toBeNull();
      expect(container.querySelectorAll('[data-selected="true"]').length).toBe(0);
    });

    fireEvent.click(screen.getByText("Fastbootd"));

    expect(onTargetChange).toHaveBeenCalledWith("fastboot");
    expect(invokeMock).toHaveBeenCalledWith("reboot_fastboot");

    await waitFor(() => {
      expect(successMock).toHaveBeenCalledWith("Rebooted to fastbootd");
      expect(screen.queryByText("Fastbootd")).toBeNull();
    });
  });
});
