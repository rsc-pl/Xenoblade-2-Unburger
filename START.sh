#!/usr/bin/env bash
set -Eeuo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PROJECT="$ROOT/toolset"
MANIFEST="$PROJECT/Cargo.toml"
TARGET="$PROJECT/target"
BINARY="$TARGET/release/xenoblade-bdat-studio"
STAMP="$TARGET/.bdat-studio-sources.sha256"
RELEASE_MODE=0
if [[ ${1:-} == --release ]]; then
  RELEASE_MODE=1
  shift
  if [[ $# -ne 0 ]]; then printf 'Usage: %s --release\n' "$0" >&2; exit 2; fi
fi

if [[ ! -f "$MANIFEST" ]]; then
  printf 'ERROR: missing %s\n' "$MANIFEST" >&2
  exit 1
fi
for executable in cargo sha256sum find sort xargs; do
  if ! command -v "$executable" >/dev/null 2>&1; then
    printf 'ERROR: required build tool not found: %s\n' "$executable" >&2
    exit 127
  fi
done

if [[ "$RELEASE_MODE" == 1 ]]; then
  if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
    printf 'ERROR: dual release requires an x86_64 Linux host.\n' >&2; exit 1
  fi
  if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    printf 'ERROR: install the MinGW-w64 cross compiler (x86_64-w64-mingw32-gcc).\n' >&2; exit 127
  fi
  if ! rustc --print target-libdir --target x86_64-pc-windows-gnu >/dev/null 2>&1 ||
     [[ ! -d "$(rustc --print target-libdir --target x86_64-pc-windows-gnu)" ]]; then
    printf 'ERROR: install Rust target x86_64-pc-windows-gnu (rustup target add x86_64-pc-windows-gnu).\n' >&2; exit 1
  fi
fi

# Hash bytes as well as filenames so unpacked files with older timestamps
# cannot cause a stale release executable to be reused.
source_signature() {
  (
    cd -- "$PROJECT"
    find src -type f -print0
    if [[ -d assets ]]; then find assets -type f -print0; fi
    for file in Cargo.toml build.rs; do
      if [[ -f "$file" ]]; then printf '%s\0' "$file"; fi
    done
  ) | (cd -- "$PROJECT" && sort -z | xargs -0 -r sha256sum | sha256sum | cut -d ' ' -f 1)
}

mkdir -p -- "$TARGET"
CURRENT="$(source_signature)"
PREVIOUS=""
if [[ -f "$STAMP" ]]; then IFS= read -r PREVIOUS < "$STAMP" || :; fi

if [[ -f "$BINARY" && "$CURRENT" != "$PREVIOUS" ]]; then
  printf 'Source changes detected; rebuilding application package.\n'
  cargo clean --release --package xenoblade-bdat-studio --manifest-path "$MANIFEST" --target-dir "$TARGET"
fi

# Always invoke Cargo: it also tracks compiler/profile/dependency fingerprints.
cargo build --release --manifest-path "$MANIFEST" --target-dir "$TARGET"
if [[ ! -f "$BINARY" || ! -x "$BINARY" ]]; then
  printf 'ERROR: Cargo returned successfully but no release binary exists at the expected path.\n' >&2
  exit 1
fi
printf '%s\n' "$CURRENT" > "$STAMP.tmp"
mv -f -- "$STAMP.tmp" "$STAMP"
if [[ "$RELEASE_MODE" == 1 ]]; then
  export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
  WINDOWS_BINARY="$TARGET/x86_64-pc-windows-gnu/release/xenoblade-bdat-studio.exe"
  WINDOWS_STAMP="$TARGET/.bdat-studio-windows-sources.sha256"
  WINDOWS_PREVIOUS=""
  if [[ -f "$WINDOWS_STAMP" ]]; then IFS= read -r WINDOWS_PREVIOUS < "$WINDOWS_STAMP" || :; fi
  if [[ -f "$WINDOWS_BINARY" && "$CURRENT" != "$WINDOWS_PREVIOUS" ]]; then
    cargo clean --release --target x86_64-pc-windows-gnu --package xenoblade-bdat-studio --manifest-path "$MANIFEST" --target-dir "$TARGET"
  fi
  cargo build --release --target x86_64-pc-windows-gnu --manifest-path "$MANIFEST" --target-dir "$TARGET"
  if [[ ! -s "$WINDOWS_BINARY" ]]; then printf 'ERROR: no Windows release binary.\n' >&2; exit 1; fi
  printf '%s\n' "$CURRENT" > "$WINDOWS_STAMP.tmp"
  mv -f -- "$WINDOWS_STAMP.tmp" "$WINDOWS_STAMP"
  mkdir -p -- "$PROJECT/dist/linux-x86_64" "$PROJECT/dist/windows-x86_64"
  install -m 755 "$BINARY" "$PROJECT/dist/linux-x86_64/xenoblade-bdat-studio"
  install -m 644 "$WINDOWS_BINARY" "$PROJECT/dist/windows-x86_64/xenoblade-bdat-studio.exe"
  printf 'Linux and Windows executables built in toolset/dist/. Backend supplied separately.\n'
  exit 0
fi
exec "$BINARY" "$@"
