import { Dialog as DialogPrimitive } from "@base-ui/react/dialog";

export type DialogChangeDetails = Parameters<
  NonNullable<DialogPrimitive.Root.Props["onOpenChange"]>
>[1];

export type DialogChangeReason = DialogChangeDetails["reason"];

export function isOutsidePressReason(reason?: DialogChangeReason) {
  return reason === "outside-press";
}

export function createDismissibleDialogRootHandler(
  onOpenChange: (open: boolean, reason?: DialogChangeReason) => void,
) {
  return (nextOpen: boolean, details: DialogChangeDetails) => {
    if (!nextOpen && isOutsidePressReason(details.reason)) {
      details.cancel();
      return;
    }

    onOpenChange(nextOpen, details.reason);
  };
}

export function applyDismissibleDialogChange(
  nextOpen: boolean,
  reason: DialogChangeReason | undefined,
  onClose: () => void,
  onOpen: () => void,
) {
  if (!nextOpen) {
    if (isOutsidePressReason(reason)) {
      return;
    }

    onClose();
    return;
  }

  onOpen();
}
