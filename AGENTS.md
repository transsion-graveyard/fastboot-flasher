# pawflash / fastboot-flasher

Rust workspace + Tauri v2 desktop app for MediaTek fastboot flashing.

## Workspace

6 crates in root `Cargo.toml`, resolver = "2":

| Package | Cargo name | Kind |
|---|---|---|
| `pawflash-cli` | `pawflash` | CLI binary — main entrypoint |
| `pawflash` (lib) | `pawflash-core` → lib `pawflash` | Core orchestration library |
| `force-fastboot` | `force-fastboot` | Library (no binary!) for MTK preloader→fastboot handoff |
| `fastboot-rs` | `fastboot_rs` | Async fastboot protocol, USB transport, sparse images |
| `mtk-scatter-parser` | `mtk_scatter_parser` | Scatter file parser + flash plan builder |
| `terminal-output` | `terminal_output` | Shared CLI terminal helpers (indicatif spinners, tables) |

**Gotcha:** `pawflash-cli` is the actual CLI binary, package name `pawflash`. The library crate is `pawflash-core` (path `pawflash/`) but its lib name is also `pawflash`. Depend on it as `pawflash = { package = "pawflash-core", path = "../pawflash" }`.

`force-fastboot` is **lib-only** — no `main.rs`. The README example `cargo run -p force-fastboot --` will fail. Use `cargo run -p pawflash -- --force-fastboot` instead.

## Tauri GUI

`pawflash-gui/` — separate workspace (`[workspace]` in `pawflash-gui/src-tauri/Cargo.toml`, deliberately excluded from root workspace members).

- Frontend: React + TypeScript + Vite + Tailwind v4 + shadcn/ui (style `base-nova`)
- Backend: Tauri v2 Rust, emits `flash-progress` and `force-fastboot-progress` events
- Dev: `cd pawflash-gui && pnpm install && cargo tauri dev`
- Frontend-only: `cd pawflash-gui && pnpm dev` (port 1420)

## Key commands

```bash
cargo build                    # whole workspace
cargo test --workspace         # all tests
cargo test -p pawflash         # single crate
cargo clippy --all-targets --all-features --locked -- -D warnings  # passes clean
cargo fmt --all                # rustfmt: reorder_imports=true, imports_granularity=Crate, group_imports=StdExternalCrate
```

No CI-enforced fmt/lint step — run manually.

## Lint rules (workspace-level, inherited)

- `missing_docs` = warn
- clippy `all` = deny (priority 10)
- `redundant_clone`, `clone_on_copy`, `manual_ok_or`, `needless_collect`, `map_unwrap_or` = deny
- `large_enum_variant` = warn

## Architecture

- `fastboot-rs` is async (tokio + nusb for USB transport)
- `force-fastboot` is sync (serialport for MTK preloader negotiation)
- `pawflash` orchestrates both, builds flash plans from scatter files, handles format/wipe/vbmeta
- Release profile (workspace): `lto = "thin"`, `strip`, `codegen-units = 1`
- Release profile (tauri): `lto = "fat"`, `opt-level = "z"`, `panic = "abort"`

## Env & setup

- Linux USB access may need udev rules; `force-fastboot` can install one automatically
- GUI: requires `pnpm` (not npm/yarn)
- CI builds Tauri bundles on ubuntu-22.04 and windows-latest; publishes releases via `workflow_dispatch` (tag input) or push to `v*` tags
