# code-style
- Use concise, prefix-free function names (e.g., `banner` not `simple_banner`, `status_line` not `simple_status_line`). Avoid overly long/suffixed names. Confidence: 0.65

# workflow
- Commit all changes frequently; prefer atomic commits after each logical change. Confidence: 0.85
- When implementing complex flows (GSI flash, scatter), build the CLI version first to get clearer output and debugging before porting to GUI. Confidence: 0.70
- Use GDB (`gdb --args target/debug/binary`) for debugging crashes and hangs. Confidence: 0.70

# cli
- Running a CLI subcommand without arguments should display --help (e.g., `pawflash scatter` equals `pawflash scatter --help`). Confidence: 0.80
- Use indicatif for CLI progress bars, optimized for small/mobile screens. Confidence: 0.65
- Before executing scatter flash, always show the plan and ask for user confirmation. Confidence: 0.75

# architecture
- Share logic between CLI and GUI; avoid separate backends. Core flash/device logic should be reusable. Confidence: 0.75
- Remove dead code and fix warnings instead of suppressing them with `#[allow(...)]`. Confidence: 0.75

# gui
See [gui/taste.md](gui/taste.md)
# flash
- Clean flash is the default mode (renamed from "firmware upgrade" to "dirty flash" for non-wipe mode). Confidence: 0.80
- After scatter flash completes, set the active slot to 'a' as a mandatory step. Confidence: 0.70
- Auto-skip partitions that fail to flash (e.g., forbidden in bootloader mode) and continue with remaining operations. Confidence: 0.70
