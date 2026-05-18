# fastboot-rs

Reusable fastboot protocol, USB transport, Android sparse image, image preparation, and execution helpers.

This crate is library-only. It exposes the pieces needed by the MTK scatter flasher/orchestrator without depending on `mtk-scatter-parser`, including `set_active`, reboot helpers, max-download-size parsing, prepared image streaming, and split raw/sparse downloads.
