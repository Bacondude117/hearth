import { useState } from "react";
import { api, type GameInfo, type PlayitStatus, type Server } from "../api";
import { copy, errorText } from "../util";
import { PlayitLink } from "./PlayitLink";
import { useToast } from "./Toasts";

const JOIN_HELP: Record<string, string> = {
  "minecraft-java": "In Minecraft: Multiplayer → Add Server, and paste this as the Server Address.",
  "minecraft-bedrock": "In Minecraft: Play → Servers → Add Server. Put the part before the colon in Server Address and the number after it in Port.",
  palworld: "In Palworld: Join Multiplayer Game, paste this address (including the :port) into the box at the bottom, and Connect.",
  terraria: "In Terraria: Multiplayer → Join via IP, pick a character, and enter this address. If it shows a :port, type the part after the colon as the port.",
};

const TUNNEL_KIND: Record<string, string> = {
  "minecraft-java": "Minecraft Java",
  "minecraft-bedrock": "Minecraft Bedrock",
};

interface Props {
  server: Server;
  game: GameInfo | undefined;
  playit: PlayitStatus | null;
  onOpenPlayit: () => void;
}

export function ShareTab({ server, game, playit, onOpenPlayit }: Props) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  if (!playit) return <p className="muted pad">Loading…</p>;
  if (!playit.supported) {
    return <p className="muted pad">Sharing through playit.gg is available on Windows.</p>;
  }
  if (!playit.linked) {
    return (
      <div className="share">
        <PlayitLink status={playit} />
        {playit.error && <p className="form-error">{playit.error}</p>}
        <LanHint server={server} />
      </div>
    );
  }

  const tunnel = playit.tunnels.find((t) => t.localPort === server.port && (t.protocol === server.protocol || t.protocol === "both"));
  const agentRunning = playit.agent.state === "running";

  const share = async () => {
    setBusy(true);
    setFailure(null);
    try {
      await api.share(server.id);
      toast("Tunnel created. It can take a minute to come online.", "success");
    } catch (e) {
      setFailure(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="share">
      {tunnel ? (
        <div className="address-card">
          <span className="stat-label">Friends join with</span>
          <div className="address-row">
            <code className="address">{tunnel.address}</code>
            <button className="btn primary" onClick={async () => (await copy(tunnel.address)) && toast("Copied!", "success")}>
              Copy
            </button>
          </div>
          <p className="muted">
            {JOIN_HELP[server.game] ?? "Give this address to your friends."}
          </p>
          {tunnel.disabledReason && <p className="form-error">playit reports: {tunnel.disabledReason}</p>}
          {server.voice && (
            <p className="muted small">
              🎙 Voice chat is shared too
              {playit.tunnels.some((t) => t.localPort === server.voice!.port) ? "" : " (setting up…)"}. Players with the Simple Voice
              Chat mod connect to it automatically.
            </p>
          )}
          {!agentRunning && (
            <div className="banner warn">
              The playit agent isn't running, so this address won't work right now.
              <button className="btn small" onClick={() => api.playitStart().catch((e) => toast(errorText(e), "error"))}>
                Start agent
              </button>
            </div>
          )}
          {server.status.state !== "running" && agentRunning && (
            <p className="muted small">The server itself is offline. Start it so people can join.</p>
          )}
          <div className="btn-row">
            <button className="btn ghost small" disabled={busy} onClick={async () => {
              setBusy(true);
              try { await api.unshare(server.id); toast("Stopped sharing", "success"); }
              catch (e) { toast(errorText(e), "error"); }
              finally { setBusy(false); }
            }}>
              Stop sharing
            </button>
            <button className="btn ghost small" onClick={onOpenPlayit}>
              playit settings
            </button>
          </div>
        </div>
      ) : (
        <div className="link-card">
          <div>
            <strong>Share {server.name} online</strong>
            <p className="muted">
              Creates a playit.gg {game?.name ?? ""} tunnel to port {server.port} on this PC and gives you an address to send to friends.
            </p>
          </div>
          <button className="btn primary" disabled={busy} onClick={share}>
            {busy ? "Creating…" : "Share online"}
          </button>
        </div>
      )}

      {failure && (
        <div className="banner bad">
          <p>
            <strong>playit couldn't create the tunnel automatically:</strong> {failure}
          </p>
          <p>You can add it yourself in a minute. Hearth will pick it up automatically:</p>
          <ol>
            <li>Open your playit.gg dashboard and choose <em>Add Tunnel</em>.</li>
            <li>
              Pick <strong>{TUNNEL_KIND[server.game] ?? `Custom ${server.protocol.toUpperCase()}`}</strong> and select this agent.
            </li>
            <li>
              Set the local port to <strong className="mono">{server.port}</strong> and create it.
            </li>
          </ol>
          <button className="btn small" onClick={() => api.openLink("https://playit.gg/account/tunnels")}>
            Open playit dashboard ↗
          </button>
        </div>
      )}
      <LanHint server={server} />
    </div>
  );
}

function LanHint({ server }: { server: Server }) {
  return (
    <p className="muted small lan-hint">
      Players on the same Wi-Fi can also join directly using this PC's local IP and port {server.port}
      {server.game === "minecraft-bedrock" ? " (they may see it under LAN Games automatically)" : ""}.
    </p>
  );
}
