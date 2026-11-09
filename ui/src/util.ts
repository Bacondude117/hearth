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

export function flavorLabel(s: Server): string {
  if (s.game === "minecraft-bedrock") return s.installedBuild ? `Bedrock ${s.installedBuild}` : "Bedrock";
  const f = s.flavor === "paper" ? "Paper" : "Vanilla";
  return `${f} ${s.version}`;
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
