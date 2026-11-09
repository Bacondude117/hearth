import { useCallback, useState } from "react";
import { api, PLAYIT_CONSOLE, type PlayitStatus, type Server } from "../api";
import { copy, errorText, statusLabel } from "../util";
import { Console } from "./Console";
import { Modal } from "./Modal";
import { PlayitLink } from "./PlayitLink";
import { useToast } from "./Toasts";

export function PlayitPage({ status, servers }: { status: PlayitStatus | null; servers: Server[] }) {
  const toast = useToast();
  const [confirmUnlink, setConfirmUnlink] = useState(false);
  const load = useCallback(() => api.playitConsole(), []);
  const act = (fn: () => Promise<void>) => fn().catch((e) => toast(errorText(e), "error"));

  if (!status) return <section className="page"><p className="muted">Loading…</p></section>;

  return (
    <section className="page">
      <header className="page-head">
        <div>
          <h1>playit.gg</h1>
          <p className="sub">Public addresses for your servers, no port forwarding needed.</p>
        </div>
        {status.linked && (
          <div className="actions">
            {status.agent.state === "running" ? (
              <button className="btn" onClick={() => act(api.playitStop)}>Stop agent</button>
            ) : (
              <button className="btn primary" onClick={() => act(api.playitStart)}>Start agent</button>
            )}
          </div>
        )}
      </header>

      {!status.supported ? (
        <p className="muted">The playit agent is available on Windows.</p>
      ) : !status.linked ? (
        <PlayitLink status={status} />
      ) : (
        <>
          <div className="stat-row">
            <div className="stat">
              <span className="stat-label">Account</span>
              <span className="pill good">Linked</span>
            </div>
            <div className="stat">
              <span className="stat-label">Agent</span>
              <span className={`pill ${status.agent.state === "running" ? "good" : status.agent.state === "crashed" ? "bad" : "idle"}`}>
                {status.agent.state === "running" ? "Running" : statusLabel(status.agent)}
              </span>
            </div>
            <div className="stat">
              <span className="stat-label">Tunnels</span>
              <span className="stat-value">{status.tunnels.length}</span>
            </div>
          </div>

          <section className="card">
            <h3>Tunnels</h3>
            {status.tunnels.length === 0 ? (
              <p className="muted">No tunnels yet. Open a server and use <em>Share online</em>.</p>
            ) : (
              <table className="tunnels">
                <thead>
                  <tr><th>Address</th><th>Goes to</th><th>Type</th><th /></tr>
                </thead>
                <tbody>
                  {status.tunnels.map((t) => {
                    const s = servers.find((x) => x.port === t.localPort);
                    return (
                      <tr key={t.id}>
                        <td className="mono">{t.address}</td>
                        <td>{s ? s.name : `port ${t.localPort ?? "?"}`}</td>
                        <td>{t.tunnelType ?? t.protocol}{t.disabledReason ? ` · ${t.disabledReason}` : ""}</td>
                        <td><button className="btn ghost small" onClick={async () => (await copy(t.address)) && toast("Copied", "success")}>Copy</button></td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
            <button className="link" onClick={() => api.openLink("https://playit.gg/account/tunnels")}>Manage on playit.gg ↗</button>
          </section>

          <section className="card grow">
            <h3>Agent log</h3>
            <Console consoleId={PLAYIT_CONSOLE} load={load} />
          </section>

          <button className="link danger-link" onClick={() => setConfirmUnlink(true)}>Unlink playit.gg account…</button>
        </>
      )}
      {status.error && <p className="form-error">{status.error}</p>}

      <Modal
        open={confirmUnlink}
        onClose={() => setConfirmUnlink(false)}
        title="Unlink playit.gg?"
        footer={
          <>
            <button className="btn ghost" onClick={() => setConfirmUnlink(false)}>Cancel</button>
            <button className="btn danger" onClick={async () => { await act(api.playitUnlink); setConfirmUnlink(false); }}>Unlink</button>
          </>
        }
      >
        <p>The agent stops and this PC forgets the account. Your tunnels stay in your playit.gg account; delete them there if you no longer need them.</p>
      </Modal>
    </section>
  );
}
