import { useState } from "react";
import { api, isActive, type PlayitStatus, type Server } from "../api";
import { copy, errorText } from "../util";
import { useToast } from "./Toasts";

/** Simple Voice Chat for Paper/Fabric/Forge/NeoForge servers. */
export function VoiceChatCard({ server, playit }: { server: Server; playit: PlayitStatus | null }) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const v = server.voice;
  const tunnel = v ? playit?.tunnels.find((t) => t.localPort === v.port && t.protocol === "udp") : undefined;
  const running = isActive(server.status);

  const run = async (fn: () => Promise<void>, done: string) => {
    setBusy(true);
    try {
      await fn();
      toast(running ? `${done} Restart the server to apply.` : done, "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="card voice-card">
      <div className="voice-head">
        <span className="voice-icon" aria-hidden>🎙</span>
        <div className="voice-text">
          <strong>Voice chat</strong>
          <p className="muted small">
            Proximity voice chat with the Simple Voice Chat {server.flavor === "paper" ? "plugin" : "mod"}. Players also need
            the Simple Voice Chat mod for Minecraft {server.version} in their game.
          </p>
        </div>
        {v ? (
          <button className="btn ghost small" disabled={busy} onClick={() => run(() => api.disableVoice(server.id), "Voice chat removed.")}>
            Remove
          </button>
        ) : (
          <button className="btn primary small" disabled={busy} onClick={() => run(() => api.enableVoice(server.id), "Voice chat added.")}>
            {busy ? "Installing…" : "Add voice chat"}
          </button>
        )}
      </div>
      {v && (
        <dl className="voice-facts">
          <dt>Version</dt>
          <dd className="mono small">{v.version}</dd>
          <dt>Voice port</dt>
          <dd className="mono small">{v.port} (UDP)</dd>
          <dt>Online</dt>
          <dd>
            {tunnel ? (
              <button className="copyable accent" onClick={async () => (await copy(tunnel.address)) && toast("Copied", "success")}>
                {tunnel.address}
              </button>
            ) : (
              <span className="muted small">Shared automatically when you use Share online.</span>
            )}
          </dd>
        </dl>
      )}
    </section>
  );
}
