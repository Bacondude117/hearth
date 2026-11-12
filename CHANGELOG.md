# Changelog

## [0.10.0] - 2026-11-12

### Added
- **Factorio** servers. On Windows, Factorio's server is part of the game, so Hearth runs the copy installed on this PC (Steam or factorio.com) with its own map, saves, mods and settings; your own game is untouched.
  - The map is created when the server is set up; Settings covers name, password, max players, whitelist, auto-pause, autosaves and more.
  - **Players** tab: make players admins, kick and ban (with a reason they see), whitelist, unban.
  - **Mods** tab: **Copy my enabled mods from Factorio** copies the mods and expansion settings (Space Age, Quality) from your game; or add mod `.zip` files.
  - Shared online through a playit UDP tunnel; friends join from Multiplayer → Connect to address.

### Changed
- The ban dialog only asks for a reason in games that show it to the banned player (Minecraft Java, Factorio).

## [0.9.0] - 2026-11-12

### Added
- **Memory limit** for Minecraft Bedrock, Palworld, Terraria and tModLoader servers, when creating them or in Settings. It's off by default (the game uses what it needs). Turned on, Windows caps the server's memory so it can't crowd out the rest of your PC; .NET servers (Terraria, tModLoader) also keep their heap under it. Minecraft Java keeps its memory slider.
- **On disk**: how much space each server's folder (server files, worlds, mods, logs) takes, shown at the top of the server page. Click it to open the folder.

## [0.8.1] - 2026-11-12

### Fixed
- Terraria and tModLoader servers ignored everything sent to their console: kick, ban, typed commands, and **Stop** (which then had to force-close the server without saving). On Windows Terraria reads its console as UTF-16, and Hearth now sends commands that way.

## [0.8.0] - 2026-11-12

### Added
- **Modded Terraria** with tModLoader: pick **tModLoader (modded)** when creating a Terraria server.
  - Hearth downloads tModLoader and the .NET runtime it needs (checksum-verified, shared between servers).
  - **Mods** tab: **Copy my enabled mods from tModLoader** copies the mods you have enabled in your own game (Steam Workshop or local), picking the build that matches the server. You can also add `.tmod` files, and turn mods on or off.
  - Players who join without the server's mods get them from the server automatically.

### Changed
- **Share online** shows the address and the port in separate boxes, each with its own Copy button, for games that ask for them separately (Terraria, Minecraft Bedrock).
- **Import Minecraft modpack** only appears when Minecraft: Java Edition is selected.

## [0.7.2] - 2026-11-12

### Fixed
- **Share online** could create a second playit tunnel for the same server if pressed again before playit listed the first one.
- A tunnel switched off on playit.gg ("disabled by user") left friends stuck on **Connecting**. The Share tab now says so, and **Replace tunnel** swaps it for a working one.

## [0.7.1] - 2026-11-11

### Fixed
- Terraria servers stayed on **Starting** and didn't count players: the real server prints its `: ` console prompt before messages (`: Server started`), which Hearth didn't expect.

## [0.7.0] - 2026-11-11

### Added
- **Terraria** servers, using Re-Logic's official dedicated server from terraria.org (no Steam needed):
  - Runs unattended: the world is created on first start from the size, difficulty and name in Settings.
  - Full console (say, save, time, playing, ...); **Stop** saves the world first.
  - Shows **Online** when the server is ready and tracks who's playing.
  - **Players** tab: kick and ban online players, unban from the ban list.
  - **Settings**: world name, size, difficulty, seed, max players, password, message of the day and port.
  - **Settings → Update** downloads the newest server after Terraria patches.
  - **Share online** uses playit's Terraria tunnel type.

## [0.6.1] - 2026-11-09

### Fixed
- Voice chat didn't connect for players joining through playit when voice chat was added to a server that was already shared: the voice tunnel was created, but its address only reached the mod on a later start. Hearth now writes the address as soon as the tunnel exists (and on every start), never clears it because of a failed playit lookup, and shows in the console which address players' voice connects to.

## [0.6.0] - 2026-11-09

### Added
- **Voice chat for Minecraft Java** (Paper, Fabric, Forge, NeoForge): one click in the Mods/Plugins tab installs Simple Voice Chat for the server's Minecraft version, gives it its own voice port, and shares it through playit alongside the game when you use Share online. Hearth points the mod at the shared address so players' clients connect automatically. Players need the Simple Voice Chat mod in their game too.

## [0.5.0] - 2026-11-09

### Added
- **Custom server icons.** Choose any picture in a server's Settings; Hearth crops it square and shows it in the sidebar and header. For Minecraft Java servers it also becomes the `server-icon.png` players see next to your server in their multiplayer list.

### Changed
- New default icons for Minecraft Java, Minecraft Bedrock and Palworld servers (original artwork).

## [0.4.2] - 2026-11-09

### Changed
- **Lighter on your PC**, so more CPU and memory go to your servers and games:
  - Hearth runs at below-normal CPU priority on Windows; servers still start at normal priority.
  - Console output is sent to the window in batches and only new lines are drawn; older lines skip layout while scrolled away. In a 5,000-line test this cut the window's CPU time by about 40%.
  - While Hearth is minimized or hidden it stops drawing console output and polling playit, and catches up when you return.
  - Fewer background threads, and fully optimized release builds.

### Fixed
- New Minecraft Java servers started with **Whitelist only** turned on, because Minecraft 26 now defaults to it. Hearth now creates them with the whitelist off.

## [0.4.1] - 2026-11-09

### Fixed
- **Share online** for Palworld failed with "tunnel type requires description". Hearth now uses playit's `palworld` tunnel type, and sends the description playit requires for custom tunnels (both verified against playit's live API).

## [0.4.0] - 2026-11-09

### Added
- **Palworld** servers. Hearth installs and updates them through Steam (SteamCMD), sets up the server's admin API with a random password, and controls the server through it:
  - Shows **Online** once the server is ready, and lists online players.
  - **Players** tab: kick, ban and unban.
  - Console box **broadcasts** messages to everyone in the game.
  - **Stop** saves the world before shutting down.
  - **Settings**: server name, password, max players, difficulty, EXP and capture rates, death penalty, PvP and more, plus every raw setting.
  - **Settings → Update** pulls the newest server from Steam after Palworld patches.
- SteamCMD support, so more Steam games can follow.

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
