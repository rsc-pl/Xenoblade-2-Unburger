# UNBURGER BDAT STUDIO

Cross-platform desktop interface for BDAT-to-JSON extraction and JSON-to-BDAT packing. The application uses an externally installed `bdat-toolset` executable for conversion; it does not implement BDAT serialization.

## Build and run

From the repository root:

- Linux: `./START.sh`
- Windows: `START.cmd`
- Linux x86-64, build both Linux and Windows executables without launching: `./START.sh --release`. Requires the Rust `x86_64-pc-windows-gnu` target and MinGW-w64 cross compiler. Outputs go under `toolset/dist/`; the platform-specific converter is supplied separately.

Both launchers invoke Cargo with a fixed `toolset/target/` build directory and start the binary only after a successful build. Source-content fingerprints trigger a rebuild even if files were extracted with older timestamps. Rust and Cargo must already be installed. Platform-specific Rust desktop dependencies may also be required. `bdat-toolset` is supplied separately and is not downloaded automatically.

The application source and generated build files reside under `toolset/`. Per-user settings and conversion logs are stored in system-specific user directories, not in the repository.

## Operations

The game selector switches the Pack and Extract workspace between Xenoblade 1, Xenoblade 2, Xenoblade 3, and Xenoblade X. Each game has separate Switch 1 and Switch 2 source and destination paths for Pack and Extract. Paths are saved automatically when edited or selected in those views. The last selected game and each game's selected platform are also saved. Settings configures only the shared backend executable. The first-run wizard can configure initial folders; on upgrade from older settings, assign the previous platform paths to their game when prompted.

Pack and Extract support single-file and batch operations. Xenoblade 1, 2, and X use the standard batch directory layout. XC3 batch packing accepts flat JSON editing folders under `game/` and `evt/` and places `evt` results into the required runtime subdirectories. Platform selection is a path profile; actual backend format compatibility depends on the installed converter and input data.

In Xenoblade X, the Pack view includes a checkbox to place `common.bdat` one directory above the selected output folder. It is enabled by default, saved independently for Switch 1 and Switch 2, and applies only to packing. Switching platforms restores that platform's choice. Older shared settings are inherited by both platforms on upgrade. For example, with destination `bdat/jp/`, the file is written to `bdat/common.bdat`; all other BDATs remain in `bdat/jp/`. Any previously generated copy in the original output folder is left untouched and reported as a warning.

The console and failed operations are displayed side by side, with live output, scrollbars, selectable text and copy-all controls. The output folder can be opened after a fully successful run. Successful packing requires a zero exit status and a nonempty output file.

## Translation workspace

Translation uses the current game's and platform's Pack Source directly. Open **Translation settings** from the compact workspace toolbar to set the optional Japanese reference folder and validation rules; both are saved separately for all eight profiles. Files are indexed in the background. Only JSON tables with a `rows` array, unique explicit IDs and the game's text field (`name` or `<DBAF43F0>`) can be edited. Japanese records match by relative path and typed ID and are read-only. Missing references are shown explicitly.

Filter files and records in continuously scrolling lists; only visible rows and a small buffer are rendered. The file browser expands multi-file directories and flattens single-file subtrees into relative paths. Search and status filters reveal matching descendants. Hover on a file to see its full relative path. The record grid shows ID, label, Japanese (when configured) and translation; select a record to edit with the Japanese reference alongside. Opt into the highlighted tag preview when needed. Save file writes only edited string values, preserves other JSON values and key order, and refuses to replace a file changed externally. Discard removes unsaved edits; switching files, profiles or workspaces is blocked until edits are saved or discarded. Validation settings support path-prefix rules and optional character and line limits; no unverified defaults are imposed on Xenoblade 1 or X. Limits are character-count estimates, not actual rendered widths.

**Validate project** reads saved JSON files in the background and lists warnings, ignored warnings, read errors and rule coverage. Use the status dropdown to filter pending files, all problems, excessive character length/read errors, excessive line count, checked clear or uncovered files. Red indicates excessive character length or read error; orange indicates excessive line count; green indicates checked and clear. Red takes precedence when both warnings exist. Gray indicates pending or uncovered/reference. Opening a file checks it; saving or changing an exception refreshes that file and retains unrelated results (stopping an active scan to avoid stale updates). Source, reference and rule changes invalidate the project snapshot. Stop validation retains partial results. Ignore warning stores an exception for a saved record, scoped to the game/platform, source, file, ID, current text and limits; Restore warning removes it. Exceptions never modify the translation.

**Mass Paste** operates on the selected table. Use `[123]: New text` for a numeric ID or `["123"]: New text` for a string ID; JSON-quoted text such as `[123]: "First line\nSecond line"` supports embedded line breaks. Preview rejects unknown, duplicate or malformed IDs and shows old/new text. Stage changes edits the in-memory document; Save file commits it. No other tables or platform profiles are modified.

**Mod version** opens the recognized version table in the selected profile. Existing UNBURGER version markers are checked before replacement; all expected rows must be present and agree on the current version. The preview changes only the version number, preserving surrounding text and controls. Stage and save explicitly. If a table, row or marker cannot be confirmed, edit manually. Changing both Switch editions requires opening each profile separately.

## Verification status

Prototype. Runtime conversion tests and platform compatibility checks are required before using it for production file replacement. Compatibility with all game revisions has not been established.
