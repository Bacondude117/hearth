import { useCallback, useEffect, useRef, useState } from "react";
import { api, onCloseRequested, onEvent, type AppInfo, type GameInfo, type PlayitStatus, type Server } from "./api";
import { Modal } from "./components/Modal";
import { ImportPackDialog } from "./components/ImportPackDialog";
import { NewServerDialog } from "./components/NewServerDialog";
import { PlayitPage } from "./components/PlayitPage";
import { ServerView } from "./components/ServerView";
import { useToast } from "./components/Toasts";
import { displayState, errorText, flavorLabel } from "./util";

type View = { kind: "server"; id: string } | { kind: "playit" } | { kind: "home" };

export function App() {
  const toast = useToast();
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [games, setGames] = useState<GameInfo[]>([]);
  const [servers, setServers] = useState<Server[] | null>(null);
  const [view, setView] = useState<View>({ kind: "home" });
  const [newOpen, setNewOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [progress, setProgress] = useState<Record<string, { stage: string; fraction: number | null }>>({});
  const [playit, setPlayit] = useState<PlayitStatus | null>(null);
  const [closing, setClosing] = useState<number | null>(null);
  const [quitting, setQuitting] = useState(false);
  const firstLoad = useRef(true);
  const serversRef = useRef<Server[] | null>(null);
  serversRef.current = servers;

  const refresh = useCallback(async () => {
    try {
      const list = await api.listServers();
      setServers(list);
      if (firstLoad.current) {
        firstLoad.current = false;
        if (list.length) setView({ kind: "server", id: list[0].id });
      }
    } catch (e) {
      toast(errorText(e), "error");
    }
  }, [toast]);

  const refreshPlayit = useCallback(() => {
    api.playitStatus().then(setPlayit, () => {});
  }, []);

  useEffect(() => {
    api.appInfo().then(setInfo, (e) => toast(errorText(e), "error"));
    api.games().then(setGames, () => {});
    void refresh();
    refreshPlayit();
    // Tunnel addresses come from playit's API; poll occasionally for state changes there.
    // Skip while Hearth is minimized or hidden; refresh as soon as it's visible again.
    const t = setInterval(() => !document.hidden && refreshPlayit(), 20000);
    const onVisible = () => !document.hidden && refreshPlayit();
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(t);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [refresh, refreshPlayit, toast]);

  useEffect(
    () =>
      onEvent((e) => {
        switch (e.type) {
          case "instancesChanged":
            void refresh();
            break;
          case "serverStatus":
            setServers((list) => list?.map((s) => (s.id === e.instanceId ? { ...s, status: e.status } : s)) ?? list);
            break;
          case "serverPlayers":
            setServers((list) => list?.map((s) => (s.id === e.instanceId ? { ...s, players: e.players } : s)) ?? list);
            break;
          case "installProgress":
            setProgress((p) => ({ ...p, [e.instanceId]: { stage: e.stage, fraction: e.fraction } }));
            break;
          case "installFinished":
            setProgress(({ [e.instanceId]: _, ...rest }) => rest);
            {
              const s = serversRef.current?.find((x) => x.id === e.instanceId);
              if (s) toast(e.error ? `Installing ${s.name} failed` : `${s.name} is ready to start`, e.error ? "error" : "success");
            }
            void refresh();
            break;
          case "playitChanged":
            refreshPlayit();
            break;
        }
      }),
    [refresh, refreshPlayit, toast],
  );

  useEffect(() => {
    let off: (() => void) | undefined;
    void onCloseRequested((n) => setClosing(n)).then((f) => (off = f));
    return () => off?.();
  }, []);

  const selected = view.kind === "server" ? servers?.find((s) => s.id === view.id) : undefined;
  // If the selected server disappeared (deleted), fall back.
  useEffect(() => {
    if (view.kind === "server" && servers && !servers.some((s) => s.id === view.id)) {
      setView(servers.length ? { kind: "server", id: servers[0].id } : { kind: "home" });
    }
  }, [servers, view]);

  const onCreated = (s: Server) => {
    setNewOpen(false);
    setImportOpen(false);
    // Add it right away so the "selected server vanished" fallback doesn't kick in.
    setServers((list) => (list?.some((x) => x.id === s.id) ? list : [...(list ?? []), s]));
    setView({ kind: "server", id: s.id });
    void refresh();
  };

  const online = servers?.filter((s) => s.status.state === "running").length ?? 0;

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">
          <span className="logo" aria-hidden />
          <span>Hearth</span>
        </div>
        <button className="btn primary new-btn" onClick={() => setNewOpen(true)}>
          ＋ New server
        </button>
        <nav className="server-list" aria-label="Servers">
          <div className="nav-label">
            Servers {servers && servers.length > 0 && <span className="muted">{online}/{servers.length} online</span>}
          </div>
          {servers?.map((s) => {
            const st = displayState(s);
            const active = view.kind === "server" && view.id === s.id;
            return (
              <button key={s.id} className={`server-item ${active ? "active" : ""}`} aria-current={active} onClick={() => setView({ kind: "server", id: s.id })}>
                <span className={`game-icon ${s.game}`} aria-hidden />
                <span className="si-text">
                  <span className="si-name">{s.name}</span>
                  <span className="si-sub">{flavorLabel(s)}</span>
                </span>
                <span className={`dot ${st.tone}`} title={st.label} />
              </button>
            );
          })}
        </nav>
        <div className="sidebar-foot">
          <button className={`nav-item ${view.kind === "playit" ? "active" : ""}`} onClick={() => setView({ kind: "playit" })}>
            <span className="playit-mark" aria-hidden />
            playit.gg
            <span className={`dot ${playit?.linked ? (playit.agent.state === "running" ? "good" : "idle") : "off"}`} />
          </button>
          <div className="version muted">v{info?.version ?? "…"}</div>
        </div>
      </aside>

      <main className="main">
        {servers === null ? (
          <div className="empty"><div className="spinner" /></div>
        ) : view.kind === "playit" ? (
          <PlayitPage status={playit} servers={servers} />
        ) : selected && info ? (
          <ServerView
            key={selected.id}
            server={selected}
            game={games.find((g) => g.id === selected.game)}
            info={info}
            playit={playit}
            progress={progress[selected.id]}
            onDeleted={() => void refresh()}
            onOpenPlayit={() => setView({ kind: "playit" })}
          />
        ) : (
          <div className="empty">
            <div className="hero">
              <span className="logo xl" aria-hidden />
              <h1>Host your first server</h1>
              <p className="muted">
                Pick a game, click create, and Hearth downloads everything it needs, including Java. Share it with friends
                through playit.gg, no router setup required.
              </p>
              <button className="btn primary big" onClick={() => setNewOpen(true)}>
                ＋ New server
              </button>
            </div>
          </div>
        )}
      </main>

      <NewServerDialog
        open={newOpen}
        games={games}
        totalMemoryMb={info?.totalMemoryMb ?? null}
        onClose={() => setNewOpen(false)}
        onCreated={onCreated}
        onImport={() => {
          setNewOpen(false);
          setImportOpen(true);
        }}
      />

      <ImportPackDialog
        open={importOpen}
        game={games.find((g) => g.id === "minecraft-java")}
        totalMemoryMb={info?.totalMemoryMb ?? null}
        onClose={() => setImportOpen(false)}
        onCreated={onCreated}
      />

      <Modal
        open={closing !== null}
        onClose={() => !quitting && setClosing(null)}
        locked={quitting}
        title="Quit Hearth?"
        footer={
          <>
            <button className="btn ghost" disabled={quitting} onClick={() => setClosing(null)}>
              Keep running
            </button>
            <button
              className="btn danger"
              disabled={quitting}
              onClick={() => {
                setQuitting(true);
                void api.quit();
              }}
            >
              {quitting ? "Stopping servers…" : "Stop servers & quit"}
            </button>
          </>
        }
      >
        <p>
          {closing === 1 ? "1 server is" : `${closing} servers are`} still running. Hearth will save the worlds and stop
          them cleanly before closing.
        </p>
      </Modal>
    </div>
  );
}
