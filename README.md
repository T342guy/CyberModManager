# CyberModManager
 A slop coded mod manager for cyberpunk 2077, because I kept getting annoyed

A small native (egui) mod manager for Cyberpunk 2077 on Linux (Steam / Proton).

## Features

- **Batch install**: pick or drag in any number of `.zip` / `.7z` / `.rar` archives. They install one after another, and one failure doesn't stop the rest.
- **Nexus Mods**:
  - Free accounts: click **Mod Manager Download** on nexusmods.com and the mod installs automatically (nxm:// handler).
  - Premium accounts: paste a mod URL or ID, pick a file, and install it directly.
- **Frameworks in one click**: *🧩 Frameworks…* installs Cyber Engine Tweaks, RED4ext, redscript, TweakXL, ArchiveXL, Codeware, Mod Settings and Input Loader from their official GitHub releases. No Nexus account is needed, and missing dependencies are added automatically (e.g. TweakXL brings RED4ext and redscript).
- **Update checker** (*⟳ Check for updates*, also runs at startup; can be turned off in Settings):
  - Frameworks are checked against their latest GitHub release and update with one click.
  - Mods linked to Nexus are checked with your API key. Premium updates in one click; free accounts get the files page opened for "Mod Manager Download".
  - Mods installed from Nexus or from Nexus-named archives (`Name-1234-1-0-...zip`) are linked automatically. Link any other mod by pasting its Nexus URL in its details panel.
- **Enable / disable**: disabled mods are moved out of the game into a holding folder, and moved back when you re-enable them.
- **Remove**: deletes the mod's files wherever they are.
- **Adopt hand-installed mods**: *Find untracked mods…* scans the mod folders for files the manager doesn't know about, groups them (CET mods, RED4ext plugins, tweaks, `.archive` mods, frameworks) and lets you adopt them so they can be disabled or removed.
- Old hand-installed copies of mod files that a managed mod replaces are set aside in `replaced/`, not restored on removal, so removing a mod really removes it.
- **Conflict handling**: when two mods ship the same file you choose who wins. Overwritten files are kept and restored when the winning mod is disabled or removed. Original game files that a mod replaces are backed up and restored the same way.
- Understands common mod layouts: game-root folders (`archive/`, `bin/`, `r6/`, `red4ext/`, `engine/`, `mods/`) even when nested inside a wrapper folder, loose `.archive`/`.xl` files, loose `.reds` scripts, loose tweak `.yaml` files and bare CET mod folders.
- Fixes letter case so files land in the existing `archive/pc/mod` folder, not a duplicate `Archive/PC/Mod`.
- Finds the game automatically in any Steam library, including ones on other drives.
- Warns you when the game is running.

## Build

Requires a Rust toolchain (edition 2024) and a C/C++ compiler (for the RAR library).

```sh
cargo build --release
./target/release/cyber-mod-manager
```

## Nexus setup

1. Copy your personal API key from <https://www.nexusmods.com/users/myaccount?tab=api>.
2. Open the **Nexus** tab, paste the key, then click **Save & check**.
3. Click **Register as nxm:// link handler**. This writes `~/.local/share/applications/cyber-mod-manager-nxm.desktop` and runs `xdg-mime`. Do it again if you move the binary.

When the manager is already open, clicked links are sent to the open window. Otherwise the link starts a new one.

## Where things live

| What | Path |
|---|---|
| Settings (incl. API key, mode 600) | `~/.config/cyber-mod-manager/config.toml` |
| Installed mod list | `~/.local/share/cyber-mod-manager/mods.json` |
| Disabled mods (default, changeable) | `~/.local/share/cyber-mod-manager/holding/<mod>/` |
| Backed-up original game files | `~/.local/share/cyber-mod-manager/backups/<mod>/` |
| Old manual-install files replaced by a mod | `~/.local/share/cyber-mod-manager/replaced/` |
| Nexus downloads | `~/.local/share/cyber-mod-manager/downloads/` |

Tip: put the holding folder on the same drive as the game. Enabling and disabling are then instant renames instead of copies.

## Development

```sh
cargo test
cargo clippy --all-targets
```
