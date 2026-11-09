# Changelog

## [0.3.2] - 2026-11-09

### Fixed
- Importing a **client-side modpack** (e.g. Fabulously Optimized) installed every client mod on the server, because such packs mark all files as server-required. Hearth now checks each mod on Modrinth and skips the ones Modrinth lists as client-only, and the import dialog warns when the pack itself is client-only.
- A server that crashed and wrote a crash report showed as **Stopped** because Minecraft exits with code 0 afterwards. It now shows as **Crashed**.

## [0.3.1] - 2026-11-09

### Fixed
- **Share online** failed with "playit: validation (failed to parse body)". playit's API rejects the request format Hearth used; Hearth now uses the tunnel endpoint and format verified against the live API.

## [0.3.0] - 2026-11-09

### Added
- **Modded servers**: new Fabric, NeoForge and Forge server types. Pick a Minecraft version and Hearth installs the recommended loader build and the right Java.
- **Import modpack**: create a server from a Modrinth `.mrpack` or a pack's server pack `.zip`. Hearth shows what it found (Minecraft version, loader, mod count) before creating anything, and asks when it can't tell.
  - `.mrpack`: only server-side mods are downloaded (checksum-verified, from Modrinth's approved hosts), then the pack's overrides and server overrides are applied.
  - Server packs: detects already-installed Forge/NeoForge, bundled installers, Fabric launchers, ServerPackCreator's `variables.txt`, or the mods themselves.
- **Mods tab** (Plugins on Paper): add jars with a file picker, see each mod's name and version, turn mods on/off, delete them.

### Changed
- Fabric servers get the vanilla server jar downloaded and verified during install instead of on first start.
- Servers created from a modpack can't change Minecraft version (it would break the mods).

## [0.2.0] - 2026-11-09

### Added
- **Players tab** for every Minecraft server: everyone who has joined, who's online, and per-player controls.
  - Role: Java servers have Player or Operator (operators get every command). Bedrock servers have Visitor, Member or Operator.
  - Whitelist (Java) / allow list (Bedrock) toggle per player, plus "Add player by name" for people who haven't joined yet.
  - Ban with an optional reason, and unban (Java).
  - Kick and change game mode for players who are online.
  - Changes apply instantly while the server runs, and are saved to the server's files while it's stopped.

## [0.1.1] - 2026-11-09

### Fixed
- Players online always showed 0 on Minecraft 26.x servers. Minecraft 26 logs join and leave messages as `System chat: Name joined the game`, which Hearth didn't recognize.

## [0.1.0] - 2026-11-09

First release.

### Added
- Minecraft: Java Edition servers: **Paper** (recommended) or **Vanilla**, any release version.
- Minecraft: Bedrock Edition servers using Mojang's official Bedrock Dedicated Server, with one-click updates that keep worlds and settings.
- Automatic Java: the right Eclipse Temurin runtime is downloaded for each Minecraft version.
- Live console with command history, colored output and a players-online count.
- Settings editor for common options (MOTD, game mode, difficulty, whitelist, port, memory) plus every raw `server.properties` key.
- Change Minecraft version for Java servers.
- playit.gg integration: link your account once, then **Share online** creates a tunnel and shows the address to send to friends. Falls back to a guided manual setup if playit refuses.
- Safe shutdown: servers are asked to save and stop. Closing Hearth with servers running asks first, and servers are terminated if Hearth itself crashes.
- Verified downloads (SHA-1/SHA-256 where the source publishes checksums).

### Known limitations
- The installer is not code-signed yet; Windows SmartScreen will warn on first run (*More info → Run anyway*).
- Bedrock: Minecraft on the same PC may need a one-time loopback exemption (see README).
