# force-fastboot

Cargo workspace (5 crates) + Tauri v2 desktop app for flashing Android/MTK devices.

## Workspace layout

| Crate | Dir | Type |
|---|---|---|
| `force-fastboot` | `force-fastboot/` | CLI binary + lib — force MTK preloader into fastboot |
| `fastboot-rs` | `fastboot-rs/` | lib only — fastboot protocol, USB transport, image helpers |
| `fastboot-flasher` | `fastboot-flasher/` | binary + lib — CLI orchestrator (scatter flash, vbmeta, format, wipe) |
| `mtk-scatter-parser` | `mtk-scatter-parser/` | binary + lib — parse MTK scatter files |
| `terminal-output` | `terminal-output/` | lib — chrome/table/progress/spinner UI helpers |
| Tauri UI | `fastboot-flasher-ui/` | React + TypeScript + Vite + Tailwind v4 + shadcn/ui + Tauri v2 |

## Commands

### Rust
- Build all: `cargo build`
- Test specific crate: `cargo test -p <crate>` (e.g., `-p fastboot-flasher`)
- Test all: `cargo test --workspace`
- No CI-enforced lint/format — run manually if needed

### Frontend (fastboot-flasher-ui/)
- `pnpm dev` — Vite dev server (port 1420, HMR 1421)
- `pnpm build` — `tsc -b && vite build`
- `pnpm lint` — ESLint
- `pnpm preview` — Preview production build

### Tauri dev
- From `fastboot-flasher-ui/`: `cargo tauri dev` (uses Vite dev server via `beforeDevCommand`)
- Release builds: CI triggers on `v*` tag pushes

### Python tests (stale — `fastboot` module not in repo)
- `python -m unittest tests/test_fastboot.py` from repo root
- `sys.path` manipulation expects a `fastboot.py` at repo root that does not exist

## Key architecture notes
- `fastboot-flasher` depends on all other workspace crates (orchestrator layer).
- `force-fastboot` CLI (`--port`, `--no-auto-udev`) handles MTK preloader USB negotiation.
- `fastboot-rs` is async (tokio, nusb); `force-fastboot` is sync (serialport).
- The Tauri backend re-exports most `fastboot-flasher` operations as `#[tauri::command]`.
- Flash progress goes through Tauri events (`flash-progress`, `force-fastboot-progress`).
- Test fixtures: `tests/fixtures/` (scatter XML, vbmeta image).
- Release profile: `lto = "fat"` + `panic = "abort"` + `opt-level = "z"` for Tauri binary.

## Gotchas
- No pre-commit hooks, no CI test/check jobs — only Tauri release builds.
- Python tests `import fastboot` but the module is absent from the repository.
- `fastboot-flasher-ui/src-tauri/Cargo.toml` has its own `[workspace]` — don't add it to root workspace.
- Bundled Linux formatter binaries at `fastboot-flasher/assets/bin/linux/`.
