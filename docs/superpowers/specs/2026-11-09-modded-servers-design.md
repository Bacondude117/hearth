# Modded Minecraft servers — design (v0.3)

## Goal

Run modded Minecraft Java servers in Hearth three ways:

1. **New modded server**: pick Fabric, Forge or NeoForge and a Minecraft version; Hearth installs
   the recommended loader build and the right Java.
2. **Import a modpack**: a Modrinth `.mrpack`, or a pack author's ready-made **server pack**
   `.zip`. Hearth inspects the file first and shows what it found before creating anything.
3. **Drop mods in**: a Mods tab (Plugins on Paper) to add, enable/disable and delete jars.

Out of scope: CurseForge export zips (`manifest.json`, needs an API key), Quilt as a server type
(Quilt packs are reported as unsupported), mod updates/browsing.

## Loaders (verified 2026-11-09)

| Loader | Versions | Install | Launch |
|---|---|---|---|
| Fabric | `meta.fabricmc.net/v2/versions/game` (stable), latest stable loader + installer | download `/v2/versions/loader/{mc}/{loader}/{installer}/server/jar` as `fabric-server-launch.jar` | `java -jar fabric-server-launch.jar nogui` (downloads the vanilla jar on first start) |
| Forge | `files.minecraftforge.net/.../promotions_slim.json` (`{mc}-recommended`, else `-latest`) | `maven.minecraftforge.net/.../forge-{mc}-{v}-installer.jar --installServer` | `java @user_jvm_args.txt @libraries/net/minecraftforge/forge/{mc}-{v}/win_args.txt nogui` |
| NeoForge | `maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge` (non-beta) | `neoforge-{v}-installer.jar --installServer` | `@libraries/net/neoforged/neoforge/{v}/win_args.txt` |

NeoForge version → Minecraft: `26.3.0.101` → `26.3`, `26.1.2.125` → `26.1.2` (new scheme, major ≥ 25);
`21.1.77` → `1.21.1`, `21.0.5` → `1.21` (old scheme). Java version always comes from Mojang's
version metadata for the Minecraft version. Unix uses `unix_args.txt`.

Launch is unified for all Java flavors: `java [@user_jvm_args.txt] -Xms -Xmx [jvm args]
(-jar <jar> | @<args file>) nogui`. Hearth's memory flags come after the pack's own file so they win.

## Pack import

`inspect(file) -> PackInfo { kind, name, packVersion, minecraft, loader, loaderVersion, modCount,
warnings, needsInput }`.

**`.mrpack`** (`modrinth.index.json`): dependencies give Minecraft + loader. Files with
`env.server == "unsupported"` are skipped. Downloads must come from Modrinth's allowed hosts
(`cdn.modrinth.com`, `github.com`, `raw.githubusercontent.com`, `gitlab.com`), are SHA-512
verified, and paths must stay inside the server folder. Then `overrides/`, then
`server-overrides/`, are copied in.

**Server `.zip`**, unpacked (single top folder stripped), detection order:
1. `libraries/**/{win,unix}_args.txt` (Forge/NeoForge already installed)
2. a Forge/NeoForge installer jar (its `install_profile.json` gives versions); run it
3. a Fabric launcher jar (`fabric-server-launch.jar`, `fabric-server-mc.*-loader.*-launcher.*.jar`)
4. `variables.txt` (ServerPackCreator: `MINECRAFT_VERSION`, `MODLOADER`, `MODLOADER_VERSION`)
5. mod metadata (`fabric.mod.json` / `neoforge.mods.toml` / `mods.toml`) → loader only

Anything still unknown sets `needsInput`; the dialog then asks for loader and Minecraft version.

The pack file is copied into the instance folder so a failed install can be retried, and removed
after a successful install. Pack servers record `pack { name, version }`; Change version is
disabled for them.

## Mods / Plugins tab

Provider declares a content folder per flavor: `mods/` (Fabric/Forge/NeoForge), `plugins/` (Paper).
Entries show display name + version from `fabric.mod.json`, `quilt.mod.json`, `*mods.toml`
(+ `MANIFEST.MF` for `${file.jarVersion}`), or `plugin.yml`/`paper-plugin.yml`.
Actions: add (native file picker, `.jar` only), enable/disable (`.jar` ↔ `.jar.disabled`), delete.
File names are validated (no path separators). Changes apply on restart.

## Desktop shell

File pickers are Rust commands using `tauri-plugin-dialog`, so the web view gets no new
permissions.

## Testing

Unit: NeoForge version mapping, Forge promos parsing, real Cobblemon `.mrpack` index (trimmed
fixture), each server-pack layout as a synthetic zip, unsafe paths/hosts rejected, mod metadata.
Live (ignored): Fabric install + boot. Forge/NeoForge installer execution is verified on Windows
by the user (the sandbox blocks running downloaded installers).
