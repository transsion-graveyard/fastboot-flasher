# pawflash

Rust workspace and Tauri desktop app for MediaTek and Android fastboot workflows.

This repository contains:

- `force-fastboot`: waits for an MTK preloader device and nudges it into fastboot mode
- `pawflash`: plans and executes safer flashing flows from MTK scatter packages
- `mtk-scatter-parser`: parses scatter files and builds flash plans without touching hardware
- `fastboot-rs`: reusable fastboot protocol and transport library
- `terminal-output`: shared CLI output helpers
- `pawflash-gui`: Tauri v2 desktop frontend for the flasher flow

## Workspace Layout

| Component | Path | Role |
| --- | --- | --- |
| `force-fastboot` | `force-fastboot/` | CLI binary + library for MTK preloader to fastboot handoff |
| `pawflash` | `pawflash/` | Main CLI orchestrator for scatter flashing, format, wipe, vbmeta, reboot |
| `mtk-scatter-parser` | `mtk-scatter-parser/` | Library + CLI for parsing scatter files and generating flash plans |
| `fastboot-rs` | `fastboot-rs/` | Library for fastboot protocol, USB transport, sparse/raw image helpers |
| `terminal-output` | `terminal-output/` | Shared terminal UI helpers |
| `pawflash-gui` | `pawflash-gui/` | React + Vite + Tailwind v4 frontend with a Tauri v2 backend |

## Requirements

- Rust toolchain
- `cargo`
- `pnpm` for the frontend/Tauri app
- A system where USB/serial access is available to the current user

On Linux, USB access may require udev rules. `force-fastboot` can attempt to install a rule automatically unless you pass `--no-auto-udev`.

## Build And Test

### Rust workspace

```bash
cargo build
cargo test --workspace
```

Run a single crate's tests:

```bash
cargo test -p pawflash
```

There is no CI-enforced formatting or lint step in this repository today, so run those manually if you want them as part of local validation.

## Main CLIs

### `force-fastboot`

Wait for an MTK preloader device and force it into fastboot mode:

```bash
cargo run -p force-fastboot -- --help
cargo run -p force-fastboot --
```

Use a specific serial port:

```bash
cargo run -p force-fastboot -- --port /dev/ttyUSB0
```

### `pawflash`

Inspect the available flows:

```bash
cargo run -p pawflash -- --help
```

Common examples:

```bash
# Build a flash plan only
cargo run -p pawflash -- inspect plan path/to/MTxxxx_Android_scatter.xml --mode dry-run

# Inspect the currently connected device
cargo run -p pawflash -- device status

# Flash a single partition image directly
cargo run -p pawflash -- flash partition boot path/to/boot.img --slot active

# Execute a firmware-upgrade style package flash
cargo run -p pawflash -- flash package path/to/MTxxxx_Android_scatter.xml --mode firmware-upgrade

# Disable vbmeta verification on both slots
cargo run -p pawflash -- flash vbmeta disable

# Wipe user data and best-effort erase metadata/cache
cargo run -p pawflash -- wipe data

# Force an MTK preloader session into fastboot
cargo run -p pawflash -- bootloader force-fastboot
```

### `mtk-scatter-parser`

Parse a scatter file without connecting to a device:

```bash
cargo run -p mtk-scatter-parser -- --help
cargo run -p mtk-scatter-parser -- path/to/MTxxxx_Android_scatter.xml --full-json
```

Useful options include `--mode`, `--slot`, `--check-images`, `--include-preloader`, and `--strict`.

## Desktop App

The desktop app lives in [`pawflash-gui`](./pawflash-gui) and uses:

- React + TypeScript + Vite
- Tailwind CSS v4
- shadcn/ui
- Tauri v2

Frontend commands:

```bash
cd pawflash-gui
pnpm install
pnpm dev
pnpm build
pnpm lint
```

Run the Tauri app in development:

```bash
cd pawflash-gui
cargo tauri dev
```

The Tauri backend emits progress events named `flash-progress` and `force-fastboot-progress` for the frontend to render live status.

## Architecture Notes

- `pawflash` is the orchestration layer and depends on the other workspace crates.
- `fastboot-rs` is async and uses `tokio` plus `nusb` for fastboot USB transport.
- `force-fastboot` is sync and uses `serialport` for MTK preloader negotiation.
- The Tauri backend re-exports most flasher operations as `#[tauri::command]`.
- `pawflash-gui/src-tauri/Cargo.toml` declares its own `[workspace]`; it should stay outside the root workspace members.
- Tauri release builds use a separate release profile with `lto = "fat"`, `panic = "abort"`, and `opt-level = "z"`.

## Credits

- `force-fastboot` is based on work from [R0rt1z2](https://github.com/R0rt1z2).
- `fastboot-rs` is based on [boardswarm/fastboot-rs](https://github.com/boardswarm/fastboot-rs).
