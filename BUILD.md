# Build Instructions

This guide covers how to set up the development environment and build HanhCute from source across different platforms.

## Prerequisites

### All Platforms

- [Rust](https://rustup.rs/) (latest stable)
- [Bun](https://bun.sh/) package manager
- [Tauri Prerequisites](https://tauri.app/start/prerequisites/)
- A C++ toolchain and [CMake](https://cmake.org/) — the speech engine
  (`transcribe-cpp`) compiles its native ggml runtime from source on first
  build. Covered by the Xcode CLT / VS Build Tools / `build-essential`
  requirements below; Windows x64 and Linux also need the Vulkan SDK for the
  GPU backend (same requirement as before).

### Platform-Specific Requirements

#### macOS

- Xcode Command Line Tools
- Install with: `xcode-select --install`

##### Intel Mac (x86_64)

Prebuilt ONNX Runtime binaries are not available for Intel Macs. Install ONNX Runtime via Homebrew and link dynamically:

```bash
brew install onnxruntime
ORT_LIB_LOCATION=$(brew --prefix onnxruntime)/lib ORT_PREFER_DYNAMIC_LINK=1 bun run tauri dev
```

The same environment variables apply for production builds:

```bash
ORT_LIB_LOCATION=$(brew --prefix onnxruntime)/lib ORT_PREFER_DYNAMIC_LINK=1 bun run tauri build
```

#### Windows

- Microsoft C++ Build Tools
- Visual Studio 2019/2022 with C++ development tools
- Or Visual Studio Build Tools 2019/2022

#### Linux

- Build essentials
- ALSA development libraries
- Install with:

  ```bash
  # Ubuntu/Debian
  sudo apt update
  sudo apt install build-essential libasound2-dev pkg-config libssl-dev libvulkan-dev vulkan-tools glslc libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libgtk-layer-shell0 libgtk-layer-shell-dev patchelf cmake

  # Fedora/RHEL
  sudo dnf groupinstall "Development Tools"
  sudo dnf install alsa-lib-devel pkgconf openssl-devel vulkan-devel \
    gtk3-devel webkit2gtk4.1-devel libappindicator-gtk3-devel librsvg2-devel \
    gtk-layer-shell gtk-layer-shell-devel \
    cmake

  # Arch Linux
  sudo pacman -S base-devel alsa-lib pkgconf openssl vulkan-devel \
    gtk3 webkit2gtk-4.1 libappindicator-gtk3 librsvg gtk-layer-shell \
    cmake
  ```

## Setup Instructions

### 1. Clone the Repository

```bash
git clone git@github.com:ductm104/Handy.git
cd Handy
```

### 2. Install Dependencies

```bash
bun install
```

### 3. Start Dev Server

```bash
bun tauri dev
```

### 4. Build for Production

```bash
bun run tauri build
```

This compiles a release binary and generates platform-specific bundles (deb, rpm, AppImage on Linux; dmg on macOS; msi on Windows).

## Linux Install (from source)

The raw binary (`src-tauri/target/release/hanhcute`) cannot run standalone — it needs Tauri resource files (tray icons, sounds, VAD model) to be co-located at the expected path.

**Install from the deb bundle** (works on any Linux distro):

```bash
cd /tmp
ar x /path/to/HanhCute/src-tauri/target/release/bundle/deb/HanhCute_*_amd64.deb data.tar.gz
tar xzf data.tar.gz
sudo cp usr/bin/hanhcute /usr/bin/
sudo cp -r usr/lib/HanhCute /usr/lib/
sudo cp usr/share/applications/HanhCute.desktop /usr/share/applications/
```

After subsequent rebuilds, only the binary needs re-copying:

```bash
sudo cp src-tauri/target/release/hanhcute /usr/bin/
```

Resources only need re-copying if they change upstream (new icons, sounds, etc.).

## Releasing a New Version

This section covers the full release workflow: bump version, build signed DMG, publish GitHub release with updater artifacts, and update the Homebrew cask.

### Prerequisites (one-time setup)

1. **Updater signing key** — already generated at `~/.tauri/hanhcute_updater.key` (private) + `.pub` (public). The public key is embedded in `src-tauri/tauri.conf.json`. `build.sh` auto-loads the private key from this path.

2. **GitHub CLI (`gh`)** — authenticated with `repo` scope:
   ```bash
   brew install gh
   gh auth login
   ```

3. **Homebrew tap repo** — `~/dev/homebrew-mytab` with remote `ductm104/homebrew-mytab` (already set up).

4. **Do NOT build inside tmux** — macOS DMG bundling uses AppleScript to position icons in the DMG window, which fails inside tmux (`Finder got an error: Application isn't running (-600)`). Use a regular terminal tab.

### Step-by-step: release version X.Y.Z

#### 1. Bump version

Update the version in **both** files:

```bash
# src-tauri/tauri.conf.json → "version": "X.Y.Z"
# package.json              → "version": "X.Y.Z"
```

Example (bumping to 0.0.85):
```bash
sed -i '' 's/"version": "0.0.84"/"version": "0.0.85"/' src-tauri/tauri.conf.json
sed -i '' 's/"version": "0.0.84"/"version": "0.0.85"/' package.json
```

#### 2. Build signed DMG + updater artifacts

Run **outside tmux**:

```bash
./build.sh --deploy
```

This produces:
- `src-tauri/target/release/bundle/dmg/HanhCute_X.Y.Z_aarch64.dmg` — DMG for new installs
- `src-tauri/target/release/bundle/macos/HanhCute.app.tar.gz` — updater bundle
- `src-tauri/target/release/bundle/macos/HanhCute.app.tar.gz.sig` — signature for updater

> If you get `failed to decode secret key: incorrect updater private key password`, verify `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in `build.sh` matches the password used when generating the key.

#### 3. Generate latest.json (updater manifest)

```bash
./scripts/generate-update-json.sh \
  src-tauri/target/release/bundle/macos/HanhCute.app.tar.gz \
  src-tauri/target/release/bundle/macos/HanhCute.app.tar.gz.sig
```

This creates `latest.json` in the repo root. The app checks `https://github.com/ductm104/Handy/releases/latest/download/latest.json` on launch to detect updates.

#### 4. Create GitHub release and upload artifacts

```bash
gh release create vX.Y.Z \
  "src-tauri/target/release/bundle/dmg/HanhCute_X.Y.Z_aarch64.dmg" \
  "src-tauri/target/release/bundle/macos/HanhCute.app.tar.gz" \
  "latest.json" \
  --title "vX.Y.Z" \
  --notes "HanhCute vX.Y.Z"
```

**Upload all 3 files** — the DMG is for new installs, the tar.gz + latest.json are for in-app auto-updates.

#### 5. Update Homebrew cask

```bash
# Compute SHA256 of the new DMG
shasum -a 256 src-tauri/target/release/bundle/dmg/HanhCute_X.Y.Z_aarch64.dmg

# Edit ~/dev/homebrew-mytab/Casks/hanhcute.rb:
#   version "X.Y.Z"
#   sha256 "<sha256-from-above>"

# Commit and push
cd ~/dev/homebrew-mytab
git add Casks/hanhcute.rb
git commit -m "hanhcute vX.Y.Z"
git push
```

Users upgrade via:
```bash
brew update && brew upgrade --cask hanhcute
```

#### 6. Commit and push the version bump

```bash
cd ~/dev/Handy
git add src-tauri/tauri.conf.json package.json
git commit -m "chore: bump version to X.Y.Z"
git push
```

### Quick reference (all steps at once)

```bash
# 1. Bump version (edit tauri.conf.json + package.json)

# 2. Build (OUTSIDE tmux)
./build.sh --deploy

# 3. Generate updater manifest
./scripts/generate-update-json.sh \
  src-tauri/target/release/bundle/macos/HanhCute.app.tar.gz \
  src-tauri/target/release/bundle/macos/HanhCute.app.tar.gz.sig

# 4. Create release
gh release create vX.Y.Z \
  "src-tauri/target/release/bundle/dmg/HanhCute_X.Y.Z_aarch64.dmg" \
  "src-tauri/target/release/bundle/macos/HanhCute.app.tar.gz" \
  "latest.json" \
  --title "vX.Y.Z" \
  --notes "HanhCute vX.Y.Z"

# 5. Update cask
shasum -a 256 src-tauri/target/release/bundle/dmg/HanhCute_X.Y.Z_aarch64.dmg
# Edit ~/dev/homebrew-mytab/Casks/hanhcute.rb → version + sha256
cd ~/dev/homebrew-mytab && git add Casks/hanhcute.rb && git commit -m "hanhcute vX.Y.Z" && git push

# 6. Push version bump
cd ~/dev/Handy && git add src-tauri/tauri.conf.json package.json && git commit -m "chore: bump version to X.Y.Z" && git push
```

### How auto-update works

- The app checks `latest.json` from the GitHub releases on launch
- If the version in `latest.json` is higher than the installed version, the footer shows "Update available"
- Clicking it downloads `HanhCute.app.tar.gz`, verifies the signature against the pubkey in `tauri.conf.json`, installs, and relaunches
- The `update_checks_enabled` setting (Debug settings → Check for Updates) controls whether checks happen

### Signing key management

- **Private key:** `~/.tauri/hanhcute_updater.key` — keep secret, never commit
- **Public key:** embedded in `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`
- **Password:** `hanhcute2024` — stored in `build.sh` as `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
- If you lose the private key or password, existing installs can't auto-update. You'd need to generate a new keypair, update the pubkey in `tauri.conf.json`, and have users manually install the new version once.

## Troubleshooting

### AppImage build fails on Arch / rolling-release distros

`linuxdeploy` bundles its own `strip` binary which is too old to process system libraries built with newer toolchains on rolling-release distros (Arch, CachyOS, Manjaro, EndeavourOS).

The error from Tauri:

```
Bundling HanhCute_*_amd64.AppImage
failed to bundle project `failed to run linuxdeploy`
```

Tauri swallows the real linuxdeploy error. To see it, run linuxdeploy manually:

```bash
cd src-tauri/target/release/bundle/appimage
~/.cache/tauri/linuxdeploy-x86_64.AppImage --appimage-extract-and-run \
  --appdir HanhCute.AppDir --plugin gtk --output appimage
```

**Workaround:** The binary, deb, and rpm bundles all build fine — only the AppImage step fails. To skip it:

```bash
bun run tauri build -- --bundles deb
```

Then install using the deb extraction method above.
