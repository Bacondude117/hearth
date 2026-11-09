# Changelog

## [0.1.0] - 2026-11-09

First release.

### Added
- Minecraft: Java Edition servers: **Paper** (recommended) or **Vanilla**, any release version.
- Minecraft: Bedrock Edition servers using Mojang's official Bedrock Dedicated Server, with
  one-click updates that keep worlds and settings.
- Automatic Java: the right Eclipse Temurin runtime is downloaded for each Minecraft version.
- Live console with command history, colored output and a players-online count.
- Settings editor for common options (MOTD, game mode, difficulty, whitelist, port, memory) plus
  every raw `server.properties` key.
- Change Minecraft version for Java servers.
- playit.gg integration: link your account once, then **Share online** creates a tunnel and shows
  the address to send to friends. Falls back to a guided manual setup if playit refuses.
- Safe shutdown: servers are asked to save and stop. Closing Hearth with servers running asks
  first, and servers are terminated if Hearth itself crashes.
- Verified downloads (SHA-1/SHA-256 where the source publishes checksums).

### Known limitations
- The installer is not code-signed yet; Windows SmartScreen will warn on first run
  (*More info → Run anyway*).
- Bedrock: Minecraft on the same PC may need a one-time loopback exemption (see README).
