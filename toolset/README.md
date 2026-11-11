# Xenoblade BDAT Studio

Cross-platform desktop interface for BDAT-to-JSON extraction and JSON-to-BDAT packing. The application uses an externally installed `bdat-toolset` executable for conversion; it does not implement BDAT serialization.

## Build and run

From the repository root:

- Linux: `./START.sh`
- Windows: `START.cmd`

Both launchers invoke Cargo with a fixed `toolset/target/` build directory and start the binary only after a successful build. Source-content fingerprints trigger a rebuild even if files were extracted with older timestamps. Rust and Cargo must already be installed. Platform-specific Rust desktop dependencies may also be required. `bdat-toolset` is supplied separately and is not downloaded automatically.

The application source and generated build files reside under `toolset/`. Per-user settings and conversion logs are stored in system-specific user directories, not in the repository.

## Operations

The interface provides Pack and Extract views, single-file and batch operations, platform-specific paths, and a first-run backend setup wizard. XC3 batch packing accepts flat JSON editing folders under `game/` and `evt/` and places `evt` results into the required runtime subdirectories.

The console and failed operations are displayed side by side, with live output, scrollbars, selectable text and copy-all controls. The output folder can be opened after a fully successful run. Successful packing requires a zero exit status and a nonempty output file.

## Verification status

Prototype. Runtime conversion tests and platform compatibility checks are required before using it for production file replacement. Compatibility with all game revisions has not been established.
