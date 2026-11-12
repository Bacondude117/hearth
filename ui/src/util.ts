import type { Server, ServerStatus } from "./api";

export function statusLabel(s: ServerStatus): string {
  switch (s.state) {
    case "stopped":
      return "Stopped";
    case "starting":
      return "Starting";
    case "running":
      return "Online";
    case "stopping":
      return "Stopping";
    case "crashed":
      return s.code === null ? "Crashed" : `Crashed (${s.code})`;
  }
}

/** Status shown in lists: install state wins over process state. */
export function displayState(s: Server): { label: string; tone: string } {
  switch (s.installState.state) {
    case "pending":
    case "installing":
      return { label: "Installing", tone: "busy" };
    case "failed":
      return { label: "Install failed", tone: "bad" };
  }
  const tone = { stopped: "idle", starting: "busy", running: "good", stopping: "busy", crashed: "bad" }[s.status.state];
  return { label: statusLabel(s.status), tone };
}

const FLAVOR_NAMES: Record<string, string> = {
  paper: "Paper",
  vanilla: "Vanilla",
  fabric: "Fabric",
  forge: "Forge",
  neoforge: "NeoForge",
};

export function flavorLabel(s: Server): string {
  if (s.game === "minecraft-bedrock") return s.installedBuild ? `Bedrock ${s.installedBuild}` : "Bedrock";
  // Steam games always run the latest build.
  if (s.version === "latest") return s.installedBuild ? `Dedicated server · ${s.installedBuild}` : "Dedicated server";
  if (s.pack) return `${s.pack.name} · ${FLAVOR_NAMES[s.flavor] ?? s.flavor} ${s.version}`;
  return `${FLAVOR_NAMES[s.flavor] ?? s.flavor} ${s.version}`;
}

export const loaderName = (flavor: string | null) => (flavor ? (FLAVOR_NAMES[flavor] ?? flavor) : "Unknown");

export function formatBytes(n: number): string {
  if (n >= 1 << 30) return `${(n / (1 << 30)).toFixed(n >= 10 * (1 << 30) ? 0 : 1)} GB`;
  if (n >= 1 << 20) return `${(n / (1 << 20)).toFixed(1)} MB`;
  if (n >= 1 << 10) return `${Math.round(n / (1 << 10))} KB`;
  return `${n} B`;
}

export function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

export async function copy(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

export function formatMb(mb: number): string {
  return mb >= 1024 ? `${+(mb / 1024).toFixed(1)} GB` : `${mb} MB`;
}
