import { useEffect, useMemo, useState } from "react";
import { api, isActive, onEvent, type AppInfo, type GameInfo, type PlayitStatus, type Server } from "../api";
import { copy, displayState, errorText, flavorLabel } from "../util";
import { Console } from "./Console";
import { SettingsTab } from "./SettingsTab";
import { ContentTab } from "./ContentTab";
import { PlayersTab } from "./PlayersTab";
import { ServerIcon } from "./ServerIcon";
import { ShareTab } from "./ShareTab";
import { useToast } from "./Toasts";

type Tab = "console" | "players" | "content" | "settings" | "share";

interface Props {
  server: Server;
  game: GameInfo | undefined;
  info: AppInfo;
  playit: PlayitStatus | null;
  progress: { stage: string; fraction: number | null } | undefined;
  onDeleted: () => void;
  onOpenPlayit: () => void;
}

export function ServerView({ server, game, info, playit, progress, onDeleted, onOpenPlayit }: Props) {
  const [tab, setTab] = useState<Tab>("console");
  const [busy, setBusy] = useState<string | null>(null);
  const toast = useToast();
  const st = displayState(server);
  const installed = server.installState.state === "installed";
  const installing = server.installState.state === "installing" || server.installState.state === "pending";
  const active = isActive(server.status);
  const loader = useMemo(() => () => api.consoleLines(server.id), [server.id]);
  const tabLabels: [Tab, string][] = [
    ["console", "Console"],
    ["players", "Players"],
    ...(server.content ? ([["content", server.content.label]] as [Tab, string][]) : []),
    ["settings", "Settings"],
    ["share", "Share online"],
  ];
  const tunnel = playit?.tunnels.find((t) => t.localPort === server.port && (t.protocol === server.protocol || t.protocol === "both"));

  useEffect(() => setTab("console"), [server.id]);

  // Toast on crash so it's noticed even from another tab.
  useEffect(
    () =>
      onEvent((e) => {
        if (e.type === "serverStatus" && e.instanceId === server.id && e.status.state === "crashed") {
          toast(`${server.name} stopped unexpectedly. Check the console for the reason.`, "error");
        }
      }),
    [server.id, server.name, toast],
  );

  const run = async (label: string, fn: () => Promise<void>) => {
    setBusy(label);
    try {
      await fn();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  const localAddress = server.port === game?.defaultPort ? "localhost" : `localhost:${server.port}`;

  return (
    <section className="server-view">
      <header className="server-head">
        <div className="title-block">
          <ServerIcon server={server} big />
          <div>
            <h1>{server.name}</h1>
            <p className="sub">
              {server.gameName} · {flavorLabel(server)}
            </p>
          </div>
        </div>
        <div className="actions">
          {installed && !active && (
            <button className="btn primary" disabled={!!busy} onClick={() => run("start", () => api.start(server.id))}>
              ▶ Start
            </button>
          )}
          {installed && active && (
            <>
              <button
                className="btn"
                disabled={!!busy || server.status.state === "stopping"}
                onClick={() => run("restart", () => api.restart(server.id))}
              >
                ↻ Restart
              </button>
              {server.status.state === "stopping" ? (
                <button className="btn danger" onClick={() => run("kill", () => api.kill(server.id))}>
                  Force stop
                </button>
              ) : (
                <button className="btn danger" disabled={!!busy} onClick={() => run("stop", () => api.stop(server.id))}>
                  ■ Stop
                </button>
              )}
            </>
          )}
          {server.installState.state === "failed" && (
            <button className="btn primary" disabled={!!busy} onClick={() => run("retry", () => api.reinstallServer(server.id, null))}>
              Retry install
            </button>
          )}
        </div>
      </header>

      <div className="stat-row">
        <div className="stat">
          <span className="stat-label">Status</span>
          <span className={`pill ${st.tone}`}>{st.label}</span>
        </div>
        <div className="stat">
          <span className="stat-label">
            Players{" "}
            <button className="link small" onClick={() => setTab("players")}>
              Manage
            </button>
          </span>
          <span className="stat-value" title={server.players.join(", ")}>
            {active ? server.players.length : "–"}
            {active && server.players.length > 0 && <small className="players">{server.players.slice(0, 4).join(", ")}{server.players.length > 4 ? "…" : ""}</small>}
          </span>
        </div>
        <div className="stat">
          <span className="stat-label">On this PC</span>
          <button className="copyable" onClick={async () => (await copy(localAddress)) && toast("Copied", "success")}>
            {localAddress}
          </button>
        </div>
        <div className="stat">
          <span className="stat-label">Online address</span>
          {tunnel ? (
            <button className="copyable accent" onClick={async () => (await copy(tunnel.address)) && toast("Address copied — send it to your friends!", "success")}>
              {tunnel.address}
            </button>
          ) : (
            <button className="link" onClick={() => setTab("share")}>
              Not shared — set up
            </button>
          )}
        </div>
      </div>

      {installing && (
        <div className="install-bar" role="status">
          <div className="install-text">
            <strong>Installing…</strong> {progress?.stage ?? "Preparing"}
            {progress?.fraction != null && ` · ${Math.round(progress.fraction * 100)}%`}
          </div>
          <progress max={1} value={progress?.fraction ?? undefined} />
        </div>
      )}
      {server.installState.state === "failed" && (
        <div className="banner bad" role="alert">
          <strong>Installation failed.</strong> {server.installState.message}
        </div>
      )}

      <nav className="tabs" role="tablist">
        {tabLabels.map(([t, label]) => (
          <button key={t} role="tab" aria-selected={tab === t} className={tab === t ? "tab active" : "tab"} onClick={() => setTab(t)}>
            {label}
          </button>
        ))}
        <span className="spacer" />
        <button className="btn ghost small" onClick={() => api.openFolder(server.id).catch((e) => toast(errorText(e), "error"))}>
          Open folder
        </button>
      </nav>

      <div className="tab-body">
        {tab === "console" && (
          <Console
            consoleId={server.id}
            load={loader}
            canSend={active}
            placeholder={game?.commandHint}
            onCommand={(line) => api.sendCommand(server.id, line).catch((e) => toast(errorText(e), "error"))}
          />
        )}
        {tab === "players" && <PlayersTab server={server} />}
        {tab === "content" && server.content && <ContentTab server={server} />}
        {tab === "settings" && <SettingsTab server={server} game={game} info={info} onDeleted={onDeleted} />}
        {tab === "share" && <ShareTab server={server} game={game} playit={playit} onOpenPlayit={onOpenPlayit} />}
      </div>
    </section>
  );
}
