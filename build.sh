#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MODEL_DIR="$ROOT_DIR/src-tauri/resources/models"
MODEL_PATH="$MODEL_DIR/silero_vad_v4.onnx"
MODEL_URL="https://blob.handy.computer/silero_vad_v4.onnx"
RUN_INSTALL=false
RUN_CLEAN=false
RUN_DEV=false
RUN_DMG=false
RUN_UPDATER_ARTIFACTS=false

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

require_command bun
require_command cargo
require_command curl

while [[ $# -gt 0 ]]; do
  case "$1" in
    --install)
      RUN_INSTALL=true
      shift
      ;;
    --clean)
      RUN_CLEAN=true
      shift
      ;;
    --dev)
      RUN_DEV=true
      shift
      ;;
    --dmg)
      RUN_DMG=true
      shift
      ;;
    --updater-artifacts)
      RUN_UPDATER_ARTIFACTS=true
      shift
      ;;
    *)
      echo "Unknown option: $1" >&2
      echo "Usage: ./build.sh [--install] [--clean] [--dev] [--dmg] [--updater-artifacts]" >&2
      exit 1
      ;;
  esac
done

mkdir -p "$MODEL_DIR"

if [[ ! -f "$MODEL_PATH" ]]; then
  echo "Downloading VAD model..."
  curl -L "$MODEL_URL" -o "$MODEL_PATH"
fi

cd "$ROOT_DIR"

if [[ ! -d "$ROOT_DIR/node_modules" ]]; then
  echo "node_modules missing, installing frontend dependencies..."
  bun install
elif [[ "$RUN_INSTALL" == true ]]; then
  echo "Installing frontend dependencies..."
  bun install
else
  echo "Skipping dependency install. Use --install to run bun install."
fi

if [[ "$RUN_CLEAN" == true ]]; then
  echo "Cleaning Rust target..."
  (cd src-tauri && cargo clean)
else
  echo "Skipping cargo clean. Use --clean to force a clean rebuild."
fi

BUILD_CACHE_DIR="$ROOT_DIR/.build-cache"
mkdir -p "$BUILD_CACHE_DIR/clang-module-cache"
export CLANG_MODULE_CACHE_PATH="${CLANG_MODULE_CACHE_PATH:-$BUILD_CACHE_DIR/clang-module-cache}"

if [[ "$RUN_DEV" == true ]]; then
  echo "Starting Tauri development mode..."
  CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}" bun run tauri dev --config '{"identifier":"com.pais.handy.dev"}'
else
  TAURI_BUILD_ARGS=(build --bundles app)

  if [[ "$RUN_DMG" == true ]]; then
    TAURI_BUILD_ARGS=(build --bundles app dmg)
  fi

  if [[ "$RUN_UPDATER_ARTIFACTS" != true ]]; then
    TAURI_BUILD_ARGS+=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
  fi

  if [[ "$RUN_DMG" == true ]]; then
    echo "Starting Tauri production build with app and DMG bundles..."
  else
    echo "Starting Tauri production build with app bundle..."
  fi

  CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}" bun run tauri -- "${TAURI_BUILD_ARGS[@]}"
fi
