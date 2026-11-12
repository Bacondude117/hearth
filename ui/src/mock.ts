// In-browser stand-in for the Rust backend, used only when the UI runs outside Tauri
// (e.g. `npm run dev` in a normal browser). It simulates installs, starts and logs.

import type { ContentEntry, GameInfo, HearthEvent, LogLine, PackInfo, PlayitStatus, Server, ServerStatus } from "./api";

const PLUGINS = { label: "Plugins", folder: "plugins", extension: "jar" };
const MODS = { label: "Mods", folder: "mods", extension: "jar" };

const games: GameInfo[] = [
  {
    id: "minecraft-java",
    name: "Minecraft: Java Edition",
    description: "The original PC edition. Players join from the Java launcher.",
    flavors: [
      { id: "paper", name: "Paper", description: "Faster than vanilla and supports Bukkit/Paper plugins. Recommended." },
      { id: "vanilla", name: "Vanilla", description: "Mojang's official server, unmodified." },
      { id: "fabric", name: "Fabric", description: "Lightweight mod loader. Most performance and utility mods." },
      { id: "neoforge", name: "NeoForge", description: "Mod loader for big content mods on newer versions." },
      { id: "forge", name: "Forge", description: "The classic mod loader. Needed for many older modpacks." },
    ],
    defaultPort: 25565,
    protocol: "tcp",
    configurableMemory: true,
    eula: {
      text: "Running a Minecraft server requires agreeing to the Minecraft End User License Agreement.",
      links: [["Minecraft EULA", "https://aka.ms/MinecraftEULA"]],
    },
    playitTunnelType: "minecraft-java",
    commandHint: "Type a command, e.g. say hello or op YourName",
    setupNote: null,
  },
  {
    id: "minecraft-bedrock",
    name: "Minecraft: Bedrock Edition",
    description: "For Windows, consoles and mobile players. Uses Mojang's official server.",
    flavors: [{ id: "bedrock", name: "Bedrock Dedicated Server", description: "Mojang's official server. Always the latest version." }],
    defaultPort: 19132,
    protocol: "udp",
    configurableMemory: false,
    eula: {
      text: "Running the Bedrock Dedicated Server requires agreeing to the Minecraft End User License Agreement and the Microsoft Privacy Policy.",
      links: [
        ["Minecraft EULA", "https://minecraft.net/terms"],
        ["Privacy Policy", "https://go.microsoft.com/fwlink/?LinkId=521839"],
      ],
    },
    playitTunnelType: "minecraft-bedrock",
    commandHint: 'Type a command, e.g. say hello or op "Your Gamertag"',
    setupNote: null,
  },
  {
    id: "palworld",
    name: "Palworld",
    description: 'Creature-collecting survival. Friends join from "Join Multiplayer Game" with your address.',
    flavors: [{ id: "steam", name: "Dedicated server", description: "Pocketpair's official dedicated server, installed through Steam." }],
    defaultPort: 8211,
    protocol: "udp",
    configurableMemory: false,
    eula: null,
    playitTunnelType: null,
    commandHint: "Broadcast a message to everyone in the game",
    setupNote: "Palworld servers need a lot of memory: 8 GB at minimum, 16 GB or more recommended. The first install downloads about 4 GB through Steam.",
  },
  {
    id: "terraria",
    name: "Terraria",
    description: "2D sandbox adventure. Friends join from Multiplayer → Join via IP with your address.",
    flavors: [{ id: "vanilla", name: "Dedicated server", description: "Re-Logic's official dedicated server from terraria.org." }],
    defaultPort: 7777,
    protocol: "tcp",
    configurableMemory: false,
    eula: null,
    playitTunnelType: "terraria",
    commandHint: "Type a command, e.g. say hello, save, or playing",
    setupNote: "The world is created the first time you start the server. Set its size, difficulty and name in Settings before you press Start.",
  },
];

const now = new Date().toISOString();
const servers: Server[] = [
  {
    id: "survival-1a2b", name: "Survival", game: "minecraft-java", flavor: "paper", version: "26.3",
    installedBuild: "build 245", port: 25565, memoryMb: 4096, javaMajor: 25, jvmArgs: [],
    installState: { state: "installed" }, createdAt: now, playitTunnelId: "t1",
    status: { state: "running" }, players: ["Alex", "Steve_2", "Notch"], protocol: "tcp", gameName: games[0].name,
    launch: { kind: "jar", jar: "server.jar" }, pack: null, content: PLUGINS, iconVersion: null, voice: null,
  },
  {
    id: "cobblemon-7a8b", name: "Cobblemon", game: "minecraft-java", flavor: "fabric", version: "1.21.1",
    installedBuild: "Fabric 0.20.1", port: 25567, memoryMb: 6144, javaMajor: 21, jvmArgs: [],
    installState: { state: "installed" }, createdAt: now, playitTunnelId: null,
    status: { state: "stopped" }, players: [], protocol: "tcp", gameName: games[0].name,
    launch: { kind: "jar", jar: "fabric-server-launch.jar" },
    pack: { name: "Cobblemon Official Modpack [Fabric]", version: "1.8.4", file: null, loaderVersion: "0.20.1" }, content: MODS, iconVersion: null, voice: null,
  },
  {
    id: "creative-3c4d", name: "Creative build", game: "minecraft-java", flavor: "vanilla", version: "26.3",
    installedBuild: null, port: 25566, memoryMb: 2048, javaMajor: 25, jvmArgs: [],
    installState: { state: "installed" }, createdAt: now, playitTunnelId: null,
    status: { state: "stopped" }, players: [], protocol: "tcp", gameName: games[0].name,
    launch: null, pack: null, content: null, iconVersion: null, voice: null,
  },
  {
    id: "bedrock-5e6f", name: "Family Bedrock", game: "minecraft-bedrock", flavor: "bedrock", version: "latest",
    installedBuild: "1.26.63.1", port: 19132, memoryMb: null, javaMajor: null, jvmArgs: [],
    installState: { state: "installed" }, createdAt: now, playitTunnelId: null,
    status: { state: "stopped" }, players: [], protocol: "udp", gameName: games[1].name,
    launch: null, pack: null, content: null, iconVersion: null, voice: null,
  },
];

servers.push({
  id: "terra-1e2f", name: "Terraria Crew", game: "terraria", flavor: "vanilla", version: "latest",
  installedBuild: "1.4.5.8", port: 7777, memoryMb: null, javaMajor: null, jvmArgs: [],
  installState: { state: "installed" }, createdAt: now, playitTunnelId: null,
  status: { state: "stopped" }, players: [], protocol: "tcp", gameName: "Terraria",
  launch: null, pack: null, content: null, iconVersion: null, voice: null,
});
servers.push({
  id: "pals-9c0d", name: "Pal Island", game: "palworld", flavor: "steam", version: "latest",
  installedBuild: "build 20481234", port: 8211, memoryMb: null, javaMajor: null, jvmArgs: [],
  installState: { state: "installed" }, createdAt: now, playitTunnelId: null,
  status: { state: "running" }, players: ["Bacon_Dude117", "PalTamer"], protocol: "udp", gameName: "Palworld",
  launch: null, pack: null, content: null, iconVersion: null, voice: null,
});

const contentStore = new Map<string, ContentEntry[]>([
  ["cobblemon-7a8b", [
    { file: "Cobblemon-fabric-1.6.1+1.21.1.jar", name: "Cobblemon", version: "1.6.1+1.21.1", enabled: true, size: 48_234_112 },
    { file: "fabric-api-0.116.6+1.21.1.jar", name: "Fabric API", version: "0.116.6+1.21.1", enabled: true, size: 2_412_331 },
    { file: "lithium-fabric-0.15.1+mc1.21.1.jar", name: "Lithium", version: "0.15.1+mc1.21.1", enabled: true, size: 812_004 },
    { file: "ferritecore-7.0.2-fabric.jar", name: "FerriteCore", version: "7.0.2", enabled: true, size: 121_830 },
    { file: "spark-1.10.119-fabric.jar.disabled", name: "spark", version: "1.10.119", enabled: false, size: 3_101_220 },
  ]],
  ["survival-1a2b", [
    { file: "EssentialsX-2.21.0.jar", name: "EssentialsX", version: "2.21.0", enabled: true, size: 3_512_004 },
    { file: "LuckPerms-Bukkit-5.5.0.jar", name: "LuckPerms", version: "5.5.0", enabled: true, size: 1_934_002 },
  ]],
]);
const mockPack: PackInfo = {
  kind: "mrpack", game: "minecraft-java", name: "Cobblemon Official Modpack [Fabric]", packVersion: "1.8.4",
  minecraft: "1.21.1", loader: "fabric", loaderVersion: "0.20.1", modCount: 36,
  notes: ["41 client-only files will be skipped."], warnings: [], needsInput: false,
};

const consoles = new Map<string, LogLine[]>();
const seqs = new Map<string, number>();
const props = new Map<string, [string, string][]>();
let linked = true;
let agent: ServerStatus = { state: "running" };
const tunnels = [
  { id: "t1", name: "Hearth: Survival", address: "ember-fox.gl.joinmc.link", localPort: 25565, protocol: "tcp", tunnelType: "minecraft-java", disabledReason: null as string | null },
];

type MockPlayer = { name: string; id: string | null; role: string; whitelisted: boolean; banned: boolean; banReason: string | null };
const playerStore = new Map<string, MockPlayer[]>([
  ["survival-1a2b", [
    { name: "Alex", id: "6ab43178-89fd-4905-97f6-0f67d9d76fd9", role: "operator", whitelisted: true, banned: false, banReason: null },
    { name: "Steve_2", id: "1c2d3e4f-0000-4000-8000-000000000002", role: "player", whitelisted: true, banned: false, banReason: null },
    { name: "Notch", id: "069a79f4-44e9-4726-a5be-fca90e38aaf5", role: "player", whitelisted: false, banned: false, banReason: null },
    { name: "Griefer99", id: "1c2d3e4f-0000-4000-8000-000000000099", role: "player", whitelisted: false, banned: true, banReason: "TNT in spawn" },
    { name: "Bacon_Dude117", id: "33f7864d-a02c-4e21-ae28-3ecf6f0fd767", role: "operator", whitelisted: true, banned: false, banReason: null },
  ]],
  ["pals-9c0d", [
    { name: "Bacon_Dude117", id: "steam_76561198000000001", role: "player", whitelisted: false, banned: false, banReason: null },
    { name: "PalTamer", id: "steam_76561198000000002", role: "player", whitelisted: false, banned: false, banReason: null },
    { name: "Griefer", id: "steam_76561198000000003", role: "player", whitelisted: false, banned: true, banReason: null },
  ]],
  ["bedrock-5e6f", [
    { name: "Cool Guy 22", id: "2535412345", role: "operator", whitelisted: true, banned: false, banReason: null },
    { name: "Kid Brother", id: null, role: "member", whitelisted: true, banned: false, banReason: null },
  ]],
]);

const icons = new Map<string, string>();
const listeners = new Set<(p: unknown) => void>();
const emit = (e: HearthEvent) => listeners.forEach((l) => l(e));

function log(id: string, text: string, stream: LogLine["stream"] = "stdout") {
  const seq = (seqs.get(id) ?? 0) + 1;
  seqs.set(id, seq);
  const line = { seq, text, stream };
  const arr = consoles.get(id) ?? [];
  arr.push(line);
  consoles.set(id, arr);
  emit({ type: "serverLog", instanceId: id, lines: [line] });
}

function setStatus(s: Server, status: ServerStatus) {
  s.status = status;
  emit({ type: "serverStatus", instanceId: s.id, status });
}

const t = () => new Date().toTimeString().slice(0, 8);

for (const l of [
  "Starting minecraft server version 26.3",
  "Loading properties",
  "Default game type: SURVIVAL",
  "Preparing level \"world\"",
  "Preparing spawn area: 100%",
  "Done (6.204s)! For help, type \"help\"",
  "Alex joined the game",
  "Steve_2 joined the game",
  "<Alex> anyone want to raid the ocean monument?",
  "Notch joined the game",
  "<Steve_2> on my way",
]) log("survival-1a2b", `[${t()} INFO]: ${l}`);
log("creative-3c4d", "Installation complete. Press Start to launch the server.", "system");

props.set("terra-1e2f", [
  ["worldname", "Terraria Crew"], ["autocreate", "2"], ["difficulty", "1"], ["maxplayers", "8"], ["password", ""],
  ["motd", "Welcome to Terraria Crew!"], ["port", "7777"], ["secure", "1"], ["upnp", "0"], ["banlist", "banlist.txt"],
]);
props.set("pals-9c0d", [
  ["ServerName", "Pal Island"], ["ServerDescription", "Friends only"], ["ServerPassword", ""], ["ServerPlayerMaxNum", "8"],
  ["Difficulty", "None"], ["DeathPenalty", "Item"], ["ExpRate", "1.500000"], ["PalCaptureRate", "1.000000"],
  ["DayTimeSpeedRate", "1.000000"], ["bIsPvP", "False"], ["bEnableFastTravel", "True"], ["PublicPort", "8211"],
  ["AdminPassword", "3f9a1c0b7e22d4a8b5c61e90"], ["RESTAPIEnabled", "True"], ["RESTAPIPort", "8212"],
]);
log("pals-9c0d", "Starting PalServer-Win64-Shipping-Cmd.exe", "system");
log("pals-9c0d", "[2026-11-09 18:02:11] [LOG] Running Palworld dedicated server on :8211");
props.set("survival-1a2b", [
  ["motd", "Survival"], ["max-players", "20"], ["gamemode", "survival"], ["difficulty", "normal"],
  ["pvp", "true"], ["online-mode", "true"], ["white-list", "false"], ["view-distance", "10"],
  ["server-port", "25565"], ["level-seed", ""], ["allow-flight", "false"], ["spawn-protection", "16"],
]);

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const find = (id: string) => {
  const s = servers.find((x) => x.id === id);
  if (!s) throw "Server not found";
  return s;
};

async function startServer(id: string) {
  const s = find(id);
  setStatus(s, { state: "starting" });
  log(id, `Starting ${s.game === "minecraft-bedrock" ? "bedrock_server.exe" : "java.exe"}`, "system");
  await sleep(600);
  log(id, `[${t()} INFO]: Starting server`);
  await sleep(900);
  log(id, `[${t()} INFO]: Done (1.512s)! For help, type "help"`);
  setStatus(s, { state: "running" });
}

async function install(s: Server) {
  s.installState = { state: "installing" };
  emit({ type: "instancesChanged" });
  const stages = s.game === "minecraft-java" ? ["Downloading Java 25", "Downloading server"] : ["Downloading Bedrock server"];
  for (const stage of stages) {
    log(s.id, `${stage}...`, "system");
    for (let f = 0; f <= 1; f += 0.1) {
      emit({ type: "installProgress", instanceId: s.id, stage, fraction: f });
      await sleep(150);
    }
  }
  s.installState = { state: "installed" };
  log(s.id, "Installation complete. Press Start to launch the server.", "system");
  emit({ type: "installFinished", instanceId: s.id, error: null });
  emit({ type: "instancesChanged" });
}

const handlers: Record<string, (a: any) => unknown> = {
  app_info: () => ({ version: "0.1.0-dev", dataDir: "C:\\Users\\you\\AppData\\Roaming\\app.hearth.desktop", totalMemoryMb: 16384, playitSupported: true }),
  games: () => games,
  versions: ({ game, flavor }) =>
    game === "palworld"
      ? [{ id: "latest", label: "Latest (Steam)", stable: true }]
      : game === "terraria"
        ? [{ id: "latest", label: "Latest (1.4.5.8)", stable: true }]
      : ["fabric", "forge", "neoforge"].includes(flavor)
      ? ["26.3", "26.2", "26.1.2", "1.21.1", "1.20.1"].map((v) => ({ id: v, label: v, stable: true }))
      : game === "minecraft-bedrock"
      ? [{ id: "latest", label: "Latest (1.26.63.1)", stable: true }]
      : flavor === "paper"
        ? [
            { id: "26.3", label: "26.3", stable: true },
            { id: "26.3-rc-3", label: "26.3-rc-3 (pre-release)", stable: false },
            { id: "26.2", label: "26.2", stable: true },
            { id: "26.1.2", label: "26.1.2", stable: true },
            { id: "1.21.11", label: "1.21.11", stable: true },
          ]
        : ["26.3", "26.2", "26.1.2", "26.1", "1.21.11", "1.21.10"].map((v) => ({ id: v, label: v, stable: true })),
  suggest_port: ({ game }) => (game === "minecraft-bedrock" ? 19134 : game === "palworld" ? 8213 : game === "terraria" ? 7778 : 25567),
  list_servers: () => servers,
  get_server: ({ id }) => find(id),
  create_server: ({ req }) => {
    const g = games.find((x) => x.id === req.game)!;
    const s: Server = {
      id: `${req.name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}-${Math.random().toString(16).slice(2, 6)}`,
      name: req.name, game: req.game, flavor: req.flavor, version: req.version, installedBuild: null,
      port: req.port ?? g.defaultPort, memoryMb: req.memoryMb, javaMajor: null, jvmArgs: [],
      installState: { state: "pending" }, createdAt: new Date().toISOString(), playitTunnelId: null,
      status: { state: "stopped" }, players: [], protocol: g.protocol, gameName: g.name,
      launch: null, pack: null, iconVersion: null, voice: null,
      content: req.flavor === "paper" ? PLUGINS : ["fabric", "forge", "neoforge"].includes(req.flavor) ? MODS : null,
    };
    servers.push(s);
    void install(s);
    return s;
  },
  reinstall_server: ({ id }) => void install(find(id)),
  delete_server: ({ id }) => {
    servers.splice(servers.findIndex((s) => s.id === id), 1);
    emit({ type: "instancesChanged" });
  },
  update_server: ({ id, name, memoryMb }) => {
    const s = find(id);
    if (name) s.name = name;
    if (memoryMb) s.memoryMb = memoryMb;
    emit({ type: "instancesChanged" });
    return s;
  },
  start_server: ({ id }) => startServer(id),
  stop_server: async ({ id }) => {
    const s = find(id);
    setStatus(s, { state: "stopping" });
    log(id, "Stopping server...", "system");
    await sleep(800);
    s.players = [];
    emit({ type: "serverPlayers", instanceId: id, players: [] });
    log(id, "Server stopped", "system");
    setStatus(s, { state: "stopped" });
  },
  restart_server: async ({ id }) => {
    await handlers.stop_server({ id });
    await startServer(id);
  },
  kill_server: ({ id }) => setStatus(find(id), { state: "stopped" }),
  send_command: ({ id, line }) => {
    log(id, `> ${line}`, "system");
    setTimeout(() => log(id, `[${t()} INFO]: There are ${find(id).players.length} of a max of 20 players online`), 200);
  },
  console_lines: ({ id }) => (id === "playit" ? [] : (consoles.get(id) ?? [])),
  get_properties: ({ id }) => props.get(id) ?? [["server-port", String(find(id).port)], ["max-players", "10"]],
  set_properties: ({ id, changes }) => {
    const cur = new Map(props.get(id) ?? []);
    for (const [k, v] of Object.entries(changes)) cur.set(k, v as string);
    props.set(id, [...cur.entries()]);
  },
  list_players: ({ id }) => {
    const s = find(id);
    const bedrock = s.game === "minecraft-bedrock";
    const list = (playerStore.get(id) ?? []).map((p) => ({ ...p, online: s.players.includes(p.name) }));
    list.sort((a, b) => Number(b.online) - Number(a.online) || a.name.localeCompare(b.name));
    return {
      caps: s.game === "palworld" || s.game === "terraria"
        ? { roles: [], canBan: true, gameModes: [], hasWhitelist: false, whitelistLabel: "", roleNeedsId: false }
        : bedrock
          ? { roles: ["visitor", "member", "operator"], canBan: false, gameModes: ["survival", "creative", "adventure", "spectator"], hasWhitelist: true, whitelistLabel: "Allow list", roleNeedsId: true }
          : { roles: ["player", "operator"], canBan: true, gameModes: ["survival", "creative", "adventure", "spectator"], hasWhitelist: true, whitelistLabel: "Whitelist", roleNeedsId: false },
      whitelistEnabled: !bedrock && s.game !== "palworld" && s.game !== "terraria",
      running: s.status.state === "running",
      players: list,
    };
  },
  player_action: ({ id, name, action }) => {
    const s = find(id);
    const list = playerStore.get(id) ?? [];
    playerStore.set(id, list);
    let p = list.find((x) => x.name.toLowerCase() === name.toLowerCase());
    if (!p) {
      p = { name, id: null, role: s.game === "minecraft-bedrock" ? "member" : "player", whitelisted: false, banned: false, banReason: null };
      list.push(p);
    }
    switch (action.type) {
      case "setRole": p.role = action.role; break;
      case "whitelist": p.whitelisted = action.on; break;
      case "ban": p.banned = true; p.banReason = action.reason; s.players = s.players.filter((n) => n !== p!.name); break;
      case "unban": p.banned = false; p.banReason = null; break;
      case "kick": s.players = s.players.filter((n) => n !== p!.name); emit({ type: "serverPlayers", instanceId: id, players: s.players }); break;
    }
    if (s.status.state === "running") log(id, `> ${action.type} ${name}`, "system");
  },
  // A generated original image stands in for the user's picture.
  enable_voice: async ({ id }) => {
    await sleep(900);
    find(id).voice = { port: 24454, file: "voicechat-fabric-2.6.27+26.3.jar", version: "fabric-2.6.27+26.3" };
    emit({ type: "instancesChanged" });
  },
  disable_voice: ({ id }) => {
    find(id).voice = null;
    emit({ type: "instancesChanged" });
  },
  pick_server_icon: ({ id }) => {
    const c = document.createElement("canvas");
    c.width = c.height = 128;
    const g = c.getContext("2d")!;
    const grad = g.createLinearGradient(0, 0, 128, 128);
    grad.addColorStop(0, "#f97316");
    grad.addColorStop(1, "#7c3aed");
    g.fillStyle = grad;
    g.fillRect(0, 0, 128, 128);
    g.fillStyle = "#fff";
    g.font = "bold 64px sans-serif";
    g.textAlign = "center";
    g.fillText(find(id).name[0], 64, 86);
    icons.set(id, c.toDataURL());
    find(id).iconVersion = Date.now();
    emit({ type: "instancesChanged" });
    return true;
  },
  clear_server_icon: ({ id }) => {
    icons.delete(id);
    find(id).iconVersion = null;
    emit({ type: "instancesChanged" });
  },
  server_icon: ({ id }) => icons.get(id) ?? null,
  pick_pack_file: () => "C:\\Users\\you\\Downloads\\Cobblemon Official Modpack [Fabric] 1.8.4.mrpack",
  inspect_pack: async () => {
    await sleep(600);
    return mockPack;
  },
  import_pack: ({ req }) => {
    const s = handlers.create_server({ req: { ...req, game: "minecraft-java", flavor: req.loader ?? mockPack.loader, version: req.minecraft ?? mockPack.minecraft } }) as Server;
    s.pack = { name: mockPack.name, version: mockPack.packVersion, file: "pack.mrpack", loaderVersion: mockPack.loaderVersion };
    return s;
  },
  list_content: ({ id }) => contentStore.get(id) ?? [],
  add_content: ({ id }) => {
    const list = contentStore.get(id) ?? [];
    contentStore.set(id, list);
    list.push({ file: "jei-1.21.1-fabric-19.21.0.jar", name: "Just Enough Items", version: "19.21.0", enabled: true, size: 1_204_331 });
    return ["jei-1.21.1-fabric-19.21.0.jar"];
  },
  set_content_enabled: ({ id, file, enabled }) => {
    const m = (contentStore.get(id) ?? []).find((x) => x.file === file);
    if (m) {
      m.enabled = enabled;
      m.file = enabled ? m.file.replace(/\.disabled$/, "") : `${m.file}.disabled`;
    }
  },
  delete_content: ({ id, file }) => contentStore.set(id, (contentStore.get(id) ?? []).filter((x) => x.file !== file)),
  open_server_folder: () => undefined,
  open_link: ({ url }) => window.open(url, "_blank"),
  share_info: ({ id }) => tunnels.find((x) => x.localPort === find(id).port) ?? null,
  share_server: async ({ id }) => {
    await sleep(700);
    const s = find(id);
    const old = tunnels.findIndex((x) => x.localPort === s.port);
    if (old >= 0 && !tunnels[old].disabledReason) return;
    if (old >= 0) tunnels.splice(old, 1);
    tunnels.push({ id: `t${tunnels.length + 1}`, name: `Hearth: ${s.name}`, address: s.protocol === "udp" ? "147.185.221.20:41234" : "quiet-otter.gl.joinmc.link", localPort: s.port, protocol: s.protocol, tunnelType: s.game, disabledReason: null });
    emit({ type: "playitChanged" });
  },
  unshare_server: ({ id }) => {
    const i = tunnels.findIndex((x) => x.localPort === find(id).port);
    if (i >= 0) tunnels.splice(i, 1);
    emit({ type: "playitChanged" });
  },
  playit_status: (): PlayitStatus => ({
    supported: true, agentInstalled: true, linked, agent, claim: null,
    agentId: linked ? "0b3a3b56-1f0e-4f43-8f52-6a6c9d3f8e11" : null, tunnels: linked ? tunnels : [], error: null,
  }),
  playit_link: () => {
    linked = true;
    emit({ type: "playitChanged" });
    return "https://playit.gg/claim/abc123";
  },
  playit_cancel_link: () => undefined,
  playit_unlink: () => {
    linked = false;
    agent = { state: "stopped" };
    emit({ type: "playitChanged" });
  },
  playit_start: () => {
    agent = { state: "running" };
    emit({ type: "playitChanged" });
  },
  playit_stop: () => {
    agent = { state: "stopped" };
    emit({ type: "playitChanged" });
  },
  playit_console: () => [
    { seq: 1, text: "Downloading the playit agent...", stream: "system" },
    { seq: 2, text: "INFO playitd: agent registered", stream: "stdout" },
    { seq: 3, text: "INFO playitd: tunnel running ember-fox.gl.joinmc.link -> 127.0.0.1:25565", stream: "stdout" },
  ],
  quit_app: () => undefined,
};

// Dev-only load generator: streams `n` console lines into the first server the way the Rust
// side delivers them (one batched event per 100 ms). Used to benchmark console rendering.
(window as unknown as { __spam: (n: number) => Promise<void> }).__spam = async (n: number) => {
  const id = "survival-1a2b";
  let sent = 0;
  while (sent < n) {
    const batch: LogLine[] = [];
    for (let i = 0; i < 250 && sent < n; i++, sent++) {
      const seq = (seqs.get(id) ?? 0) + 1;
      seqs.set(id, seq);
      batch.push({ seq, text: `[12:00:00 INFO]: [mod${sent % 40}] Loaded ${sent} things in ${sent % 97}ms`, stream: "stdout" });
    }
    consoles.set(id, [...(consoles.get(id) ?? []), ...batch]);
    emit({ type: "serverLog", instanceId: id, lines: batch });
    await sleep(100);
  }
};

export function createMockBackend() {
  return {
    invoke: async <T,>(cmd: string, args?: Record<string, unknown>): Promise<T> => {
      await sleep(60);
      const h = handlers[cmd];
      if (!h) throw `mock: unknown command ${cmd}`;
      // Deep copy so React sees new objects, like real IPC.
      return structuredClone(await h(args ?? {})) as T;
    },
    listen: async (name: string, cb: (p: unknown) => void) => {
      if (name !== "hearth") return () => {};
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
  };
}
