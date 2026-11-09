# Hearth — design (v1)

*Working name; rename freely.*

## Goal

A polished Windows desktop app you can hand to friends: one dashboard to install, run and
manage game servers on your own PC, with optional playit.gg tunnels so others can join
without port forwarding. Multi-game by design; v1 ships Minecraft Java (Vanilla, Paper) and
Minecraft Bedrock (official Bedrock Dedicated Server).

### Decisions (from brainstorming)

| Topic | Decision |
|---|---|
| Audience | Shareable, polished; non-technical friends should manage |
| Platform | Windows (core also compiles/runs on Linux for development and tests) |
| Stack | Rust + Tauri v2, React + TypeScript (Vite) UI |
| Dashboard reach | Desktop window only (no web server, no auth) |
| Java flavors | Vanilla, Paper |
| Bedrock | Official BDS, latest release |
| Java runtime | Auto-downloaded Eclipse Temurin JRE, chosen per Minecraft version |
| Tunneling | playit.gg, per server, optional |

### Non-goals for v1

Modded loaders (Fabric/Forge), modpacks, Steam games, backups/scheduling, remote access,
multiple hosts, auto-update of the app itself. The game catalog is built so Steam games can
be added as new providers without touching the rest.

## Architecture

```
hearth/
  crates/hearth-core/   all logic, UI-agnostic, unit + integration tested
  src-tauri/            thin Tauri shell: commands -> core, core events -> UI
  ui/                   React + TS front end
```

`hearth-core` knows nothing about Tauri. The shell implements one trait (`EventSink`) to
forward events to the webview. This keeps the logic testable on CI without a GUI.

### Core units

| Module | Responsibility |
|---|---|
| `paths` | Data layout under the Tauri app-data dir (`%APPDATA%\app.hearth.desktop`) (`instances/`, `runtimes/`, `tools/`, `playit/`) |
| `http` | Shared client (User-Agent required by PaperMC), streamed downloads with progress + SHA-1/SHA-256 verification |
| `archive` | Zip extraction with path-traversal guard, optional strip of the top-level folder, preserve-list for updates |
| `properties` | `server.properties` read/edit that preserves comments and order |
| `instance` | `Instance` record + `InstanceStore` (one `instance.json` per server folder) |
| `games` | `GameProvider` trait + registry. Providers: `minecraft_java`, `minecraft_bedrock` |
| `java` | `JavaRuntimes::ensure(major)` downloads Temurin JRE from Adoptium, verifies SHA-256 |
| `process` | Runs one server: spawn, stdin commands, log ring buffer, status, graceful stop (`stop` then kill after timeout), player join/leave parsing |
| `manager` | Facade the UI calls: create/install/update/delete/start/stop/restart/command/properties |
| `playit` | Download `playitd`, claim flow, run daemon, list/create/delete tunnels via playit API |

### GameProvider contract

```rust
trait GameProvider {
    fn info(&self) -> GameInfo;                         // id, name, flavors, default port, protocol
    async fn versions(&self, ctx, flavor) -> Vec<GameVersion>;
    async fn install(&self, ctx, instance, progress) -> InstallOutcome;  // also used for updates
    fn launch(&self, ctx, instance) -> LaunchSpec;      // program, args, env, cwd
    fn parse_line(&self, line) -> Option<LineEvent>;    // Ready / PlayerJoined / PlayerLeft
    fn stop_command(&self) -> &str;
}
```

Adding Valheim later = one new file implementing this (SteamCMD install, launch args, log
patterns) plus a registry line.

### Upstream sources (verified 2026-11-09)

- Vanilla: `piston-meta.mojang.com/mc/game/version_manifest_v2.json` → per-version JSON gives
  `downloads.server.{url,sha1}` and `javaVersion.majorVersion`.
- Paper: `fill.papermc.io/v3/projects/paper` (versions), `.../versions/{v}` (min Java + recommended
  JVM flags), `.../versions/{v}/builds/latest` (`downloads["server:default"].{url,checksums.sha256}`).
- Java: `api.adoptium.net/v3/assets/latest/{major}/hotspot?os=windows&architecture=x64&image_type=jre`.
- Bedrock: `net-secondary.web.minecraft-services.net/api/v1.0/download/links` → `serverBedrockWindows`.
  No checksum is published; version is parsed from the file name. Only the latest is offered.
- playit: `github.com/playit-cloud/playit-agent/releases/latest/download/playitd-windows-x86_64.exe`;
  API `https://api.playit.gg` with `Authorization: Agent-Key <secret>`; responses are
  `{"status":"success"|"fail"|"error","data":...}`.

## Data flow

1. **Create**: wizard → `manager.create(spec)` → instance folder + `instance.json` →
   background install task emits `install-progress` events → status `Installed`.
2. **Start**: port-in-use check → provider `launch()` → `process` spawns with piped stdio,
   no console window → `server-log` / `server-status` / `server-players` events.
3. **Stop**: write `stop` to stdin, wait up to 45 s, then kill. App exit stops all servers.
4. **Tunnel**: user links playit once (claim URL opens in browser, app polls until approved,
   stores secret in `playit/playit.toml`). App runs `playitd --secret-path ...` as a child.
   "Share online" on a server creates a tunnel (`minecraft-java` TCP / `minecraft-bedrock`
   UDP) pointing at `127.0.0.1:<port>`; the public address from `/v1/agents/rundata` is shown
   with a copy button. If the API refuses (e.g. account needs verification), the UI shows the
   reason and a link to the playit dashboard.

## Error handling

- Every core error is a `hearth_core::Error` with a user-readable message; commands return
  `Result<T, String>` to the UI, which shows toasts.
- Downloads go to `*.part` then rename after hash verification; failed installs leave the
  instance in `InstallFailed` with the message and a Retry button.
- Server crash (non-zero exit while not stopping) → `Crashed` status with exit code; last log
  lines stay visible.
- Bedrock updates preserve `server.properties`, `permissions.json`, `allowlist.json`,
  `whitelist.json`, `worlds/`.

## UI

Sidebar: server list with status dots + "New server" + "playit" + "Settings".
Server page tabs: **Console** (live log, command box), **Settings** (common properties as
form + raw table, memory for Java), **Share** (playit tunnel), **Files** (open folder).
Header: Start/Stop/Restart, status, players online, address to copy.
Dark theme, light theme via `prefers-color-scheme`.

## Testing

- Unit: properties round-trip, manifest/API parsing against recorded fixtures, version
  sorting, line parsing, zip extraction guard, port checks, playit envelope parsing.
- Integration: process manager against a fake server script (ready line, command echo,
  graceful stop, crash, kill on timeout).
- Live (ignored by default, `cargo test -- --ignored`): resolve real versions/URLs.
- CI: GitHub Actions on `windows-latest` runs core tests and builds the NSIS/MSI installer.
