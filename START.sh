#!/usr/bin/env bash
set -Eeuo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PROJECT="$ROOT/toolset"
MANIFEST="$PROJECT/Cargo.toml"
TARGET="$PROJECT/target"
BINARY="$TARGET/release/xenoblade-bdat-studio"
STAMP="$TARGET/.bdat-studio-sources.sha256"

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
exec "$BINARY" "$@"
