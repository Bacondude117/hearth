// Typed bridge to the Rust commands in src-tauri/src/commands.rs.
// Outside Tauri (plain `npm run dev` in a browser) a mock backend is used so the UI can be
// developed and screenshotted without the desktop shell.

export type InstallState =
  | { state: "pending" }
  | { state: "installing" }
  | { state: "installed" }
  | { state: "failed"; message: string };

export type ServerStatus =
  | { state: "stopped" }
  | { state: "starting" }
  | { state: "running" }
  | { state: "stopping" }
  | { state: "crashed"; code: number | null };

export type Protocol = "tcp" | "udp";

export interface Server {
  id: string;
  name: string;
  game: string;
  flavor: string;
  version: string;
  installedBuild: string | null;
  port: number;
  memoryMb: number | null;
  javaMajor: number | null;
  jvmArgs: string[];
  installState: InstallState;
  createdAt: string;
  playitTunnelId: string | null;
  status: ServerStatus;
  players: string[];
  protocol: Protocol;
  gameName: string;
  launch: { kind: "jar"; jar: string } | { kind: "argsFile"; argsFile: string } | null;
  pack: { name: string; version: string | null; file: string | null; loaderVersion: string | null } | null;
  /** The server's mods/plugins folder, if it has one. */
  content: { label: string; folder: string; extension: string } | null;
  /** Changes when the custom icon changes; null = the game's default icon. */
  iconVersion: number | null;
}

export interface PackInfo {
  kind: "mrpack" | "serverPack";
  game: string;
  name: string;
  packVersion: string | null;
  minecraft: string | null;
  loader: string | null;
  loaderVersion: string | null;
  modCount: number;
  notes: string[];
  warnings: string[];
  needsInput: boolean;
}

export interface ImportPack {
  file: string;
  name: string;
  port: number | null;
  memoryMb: number | null;
  acceptEula: boolean;
  minecraft: string | null;
  loader: string | null;
}

export interface ContentEntry {
  file: string;
  name: string;
  version: string | null;
  enabled: boolean;
  size: number;
}

export interface GameInfo {
  id: string;
  name: string;
  description: string;
  flavors: { id: string; name: string; description: string }[];
  defaultPort: number;
  protocol: Protocol;
  configurableMemory: boolean;
  eula: { text: string; links: [string, string][] } | null;
  playitTunnelType: string | null;
  commandHint: string;
  setupNote: string | null;
}

export interface GameVersion {
  id: string;
  label: string;
  stable: boolean;
}

export interface LogLine {
  seq: number;
  text: string;
  stream: "stdout" | "stderr" | "system";
}

export interface TunnelInfo {
  id: string;
  name: string;
  address: string;
  localPort: number | null;
  protocol: string;
  tunnelType: string | null;
  disabledReason: string | null;
}

export type Role = "player" | "visitor" | "member" | "operator";

export interface PlayerEntry {
  name: string;
  id: string | null;
  online: boolean;
  role: Role;
  whitelisted: boolean;
  banned: boolean;
  banReason: string | null;
}

export interface PlayersView {
  caps: {
    roles: Role[];
    canBan: boolean;
    gameModes: string[];
    hasWhitelist: boolean;
    whitelistLabel: string;
    roleNeedsId: boolean;
  };
  whitelistEnabled: boolean;
  running: boolean;
  players: PlayerEntry[];
}

export type PlayerAction =
  | { type: "setRole"; role: Role }
  | { type: "whitelist"; on: boolean }
  | { type: "ban"; reason: string | null }
  | { type: "unban" }
  | { type: "kick"; reason: string | null }
  | { type: "gameMode"; mode: string };

export interface PlayitStatus {
  supported: boolean;
  agentInstalled: boolean;
  linked: boolean;
  agent: ServerStatus;
  claim: { url: string; state: string } | null;
  agentId: string | null;
  tunnels: TunnelInfo[];
  error: string | null;
}

export interface AppInfo {
  version: string;
  dataDir: string;
  totalMemoryMb: number | null;
  playitSupported: boolean;
}

export interface CreateServer {
  name: string;
  game: string;
  flavor: string;
  version: string;
  port: number | null;
  memoryMb: number | null;
  acceptEula: boolean;
}

export type HearthEvent =
  | { type: "installProgress"; instanceId: string; stage: string; fraction: number | null }
  | { type: "installFinished"; instanceId: string; error: string | null }
  | { type: "serverLog"; instanceId: string; lines: LogLine[] }
  | { type: "serverStatus"; instanceId: string; status: ServerStatus }
  | { type: "serverPlayers"; instanceId: string; players: string[] }
  | { type: "instancesChanged" }
  | { type: "playitChanged" };

/// Console id the playit agent's log lines arrive under.
export const PLAYIT_CONSOLE = "playit";

type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = (event: string, cb: (payload: unknown) => void) => Promise<() => void>;

interface Backend {
  invoke: Invoke;
  listen: Listen;
}

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

let backendPromise: Promise<Backend> | null = null;

function backend(): Promise<Backend> {
  backendPromise ??= (async () => {
    if (isTauri) {
      const core = await import("@tauri-apps/api/core");
      const ev = await import("@tauri-apps/api/event");
      return {
        invoke: core.invoke as Invoke,
        listen: (name, cb) => ev.listen(name, (e) => cb(e.payload)),
      };
    }
    if (import.meta.env.DEV) {
      const mock = await import("./mock");
      return mock.createMockBackend();
    }
    throw new Error("Hearth's interface must run inside the desktop app.");
  })();
  return backendPromise;
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await (await backend()).invoke<T>(cmd, args);
  } catch (e) {
    // Rust commands reject with a plain, user-readable string.
    throw new Error(typeof e === "string" ? e : e instanceof Error ? e.message : String(e));
  }
}

export const api = {
  appInfo: () => call<AppInfo>("app_info"),
  games: () => call<GameInfo[]>("games"),
  versions: (game: string, flavor: string) => call<GameVersion[]>("versions", { game, flavor }),
  suggestPort: (game: string) => call<number>("suggest_port", { game }),
  listServers: () => call<Server[]>("list_servers"),
  getServer: (id: string) => call<Server>("get_server", { id }),
  createServer: (req: CreateServer) => call<Server>("create_server", { req }),
  reinstallServer: (id: string, version: string | null) => call<void>("reinstall_server", { id, version }),
  deleteServer: (id: string, removeTunnel: boolean) => call<void>("delete_server", { id, removeTunnel }),
  updateServer: (id: string, name: string | null, memoryMb: number | null) =>
    call<Server>("update_server", { id, name, memoryMb }),
  start: (id: string) => call<void>("start_server", { id }),
  stop: (id: string) => call<void>("stop_server", { id }),
  restart: (id: string) => call<void>("restart_server", { id }),
  kill: (id: string) => call<void>("kill_server", { id }),
  sendCommand: (id: string, line: string) => call<void>("send_command", { id, line }),
  consoleLines: (id: string) => call<LogLine[]>("console_lines", { id }),
  getProperties: (id: string) => call<[string, string][]>("get_properties", { id }),
  setProperties: (id: string, changes: Record<string, string>) => call<void>("set_properties", { id, changes }),
  pickServerIcon: (id: string) => call<boolean>("pick_server_icon", { id }),
  clearServerIcon: (id: string) => call<void>("clear_server_icon", { id }),
  serverIcon: (id: string) => call<string | null>("server_icon", { id }),
  pickPackFile: () => call<string | null>("pick_pack_file"),
  inspectPack: (path: string) => call<PackInfo>("inspect_pack", { path }),
  importPack: (req: ImportPack) => call<Server>("import_pack", { req }),
  listContent: (id: string) => call<ContentEntry[]>("list_content", { id }),
  addContent: (id: string) => call<string[]>("add_content", { id }),
  setContentEnabled: (id: string, file: string, enabled: boolean) => call<void>("set_content_enabled", { id, file, enabled }),
  deleteContent: (id: string, file: string) => call<void>("delete_content", { id, file }),
  listPlayers: (id: string) => call<PlayersView>("list_players", { id }),
  playerAction: (id: string, name: string, action: PlayerAction) => call<void>("player_action", { id, name, action }),
  openFolder: (id: string) => call<void>("open_server_folder", { id }),
  openLink: (url: string) => call<void>("open_link", { url }),
  shareInfo: (id: string) => call<TunnelInfo | null>("share_info", { id }),
  share: (id: string) => call<void>("share_server", { id }),
  unshare: (id: string) => call<void>("unshare_server", { id }),
  playitStatus: () => call<PlayitStatus>("playit_status"),
  playitLink: () => call<string>("playit_link"),
  playitCancelLink: () => call<void>("playit_cancel_link"),
  playitUnlink: () => call<void>("playit_unlink"),
  playitStart: () => call<void>("playit_start"),
  playitStop: () => call<void>("playit_stop"),
  playitConsole: () => call<LogLine[]>("playit_console"),
  quit: () => call<void>("quit_app"),
};

// ---- events ---------------------------------------------------------------------------------

type Handler = (e: HearthEvent) => void;
const handlers = new Set<Handler>();
let wired = false;

/** Subscribes to backend events. Returns an unsubscribe function. */
export function onEvent(h: Handler): () => void {
  handlers.add(h);
  if (!wired) {
    wired = true;
    void backend().then((b) => b.listen("hearth", (p) => handlers.forEach((fn) => fn(p as HearthEvent))));
  }
  return () => handlers.delete(h);
}

/** Fired when the user closes the window while servers are running. */
export async function onCloseRequested(cb: (running: number) => void): Promise<() => void> {
  return (await backend()).listen("close-requested", (p) => cb(p as number));
}

export const isActive = (s: ServerStatus) => s.state === "starting" || s.state === "running" || s.state === "stopping";
