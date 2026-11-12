import { useCallback, useEffect, useState } from "react";
import { api, isActive, type ContentEntry, type PlayitStatus, type Server } from "../api";
import { errorText, formatBytes } from "../util";
import { Modal } from "./Modal";
import { useToast } from "./Toasts";
import { VoiceChatCard } from "./VoiceChatCard";

/** Mods (Fabric/Forge/NeoForge/tModLoader) or Plugins (Paper): add, enable/disable, delete files. */
export function ContentTab({ server, playit }: { server: Server; playit: PlayitStatus | null }) {
  const toast = useToast();
  const folder = server.content!;
  const noun = folder.label === "Plugins" ? "plugin" : "mod";
  const [items, setItems] = useState<ContentEntry[] | null>(null);
  const [filter, setFilter] = useState("");
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<ContentEntry | null>(null);
  const [changed, setChanged] = useState(false);
  const running = isActive(server.status);

  const load = useCallback(() => {
    api.listContent(server.id).then(setItems, (e) => toast(errorText(e), "error"));
  }, [server.id, toast]);

  useEffect(() => {
    setItems(null);
    setChanged(false);
    load();
  }, [load]);

  const run = async (fn: () => Promise<void>) => {
    try {
      await fn();
      setChanged(true);
    } catch (e) {
      toast(errorText(e), "error");
    }
    load();
  };

  const add = async () => {
    setBusy(true);
    try {
      const added = await api.addContent(server.id);
      if (added.length) {
        toast(added.length === 1 ? `Added ${added[0]}` : `Added ${added.length} ${noun}s`, "success");
        setChanged(true);
      }
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
      load();
    }
  };

  const importFromGame = async () => {
    setBusy(true);
    try {
      const r = await api.importClientContent(server.id);
      if (r.copied.length) {
        toast(`Copied ${r.copied.length} ${noun}${r.copied.length === 1 ? "" : "s"} from your game`, "success");
        setChanged(true);
      } else if (!r.missing.length) {
        toast(`No ${noun}s are enabled in your game.`, "info");
      }
      if (r.missing.length) {
        toast(`Couldn't find a copy for this server version: ${r.missing.join(", ")}`, "error");
      }
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
      load();
    }
  };

  if (server.installState.state !== "installed") {
    return <p className="muted pad">{folder.label} can be managed once the server is installed.</p>;
  }

  const shown = (items ?? []).filter(
    (m) => !filter || m.name.toLowerCase().includes(filter.toLowerCase()) || m.file.toLowerCase().includes(filter.toLowerCase()),
  );
  const enabled = items?.filter((m) => m.enabled).length ?? 0;

  return (
    <div className="content-tab">
      <VoiceChatCard server={server} playit={playit} />
      <div className="players-top">
        <div className="add-player">
          <button className="btn primary" onClick={add} disabled={busy}>
            ＋ Add {noun}s…
          </button>
          {folder.importLabel && (
            <button className="btn ghost" onClick={importFromGame} disabled={busy}>
              {folder.importLabel}
            </button>
          )}
          {items && items.length > 6 && (
            <input value={filter} onChange={(e) => setFilter(e.target.value)} placeholder={`Filter ${items.length} ${noun}s`} aria-label={`Filter ${noun}s`} />
          )}
        </div>
        <p className="muted small apply-note">
          {items ? `${enabled} of ${items.length} enabled` : ""}
        </p>
      </div>

      {changed && running && (
        <div className="banner info">
          {folder.label} changed. <strong>Restart the server</strong> to apply.
        </div>
      )}

      {!items ? (
        <p className="muted">Loading…</p>
      ) : items.length === 0 ? (
        <div className="empty-players">
          <p>
            <strong>No {noun}s yet.</strong>
          </p>
          {folder.extension === "tmod" ? (
            <p className="muted">
              Enable mods in tModLoader on this PC, then use <strong>{folder.importLabel}</strong>, or add .tmod files directly.
              Players who join without them get the mods from the server automatically.
            </p>
          ) : (
            <p className="muted">
              Add .jar files built for this server's {server.flavor === "paper" ? "Paper" : "loader"} and Minecraft version {server.version}.
              {noun === "mod" && " Only server-side or both-sides mods do anything here; client-only mods (shaders, minimaps) belong on players' PCs."}
            </p>
          )}
        </div>
      ) : (
        <div className="table-wrap">
          <table className="players-table">
            <thead>
              <tr>
                <th>{folder.label === "Plugins" ? "Plugin" : "Mod"}</th>
                <th>Version</th>
                <th>Enabled</th>
                <th className="right" />
              </tr>
            </thead>
            <tbody>
              {shown.map((m) => (
                <tr key={m.file} className={m.enabled ? undefined : "disabled-row"}>
                  <td>
                    <div className="mod-name">{m.name}</div>
                    <div className="muted small mono">
                      {m.file} · {formatBytes(m.size)}
                    </div>
                  </td>
                  <td className="mono small">{m.version ?? "–"}</td>
                  <td>
                    <input
                      type="checkbox"
                      role="switch"
                      className="switch"
                      checked={m.enabled}
                      aria-label={`Enable ${m.name}`}
                      onChange={(e) => run(() => api.setContentEnabled(server.id, m.file, e.target.checked))}
                    />
                  </td>
                  <td className="right">
                    <button className="btn small danger" onClick={() => setConfirm(m)}>
                      Delete
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <Modal
        open={confirm !== null}
        onClose={() => setConfirm(null)}
        title={`Delete ${confirm?.name ?? ""}?`}
        footer={
          <>
            <button className="btn ghost" onClick={() => setConfirm(null)}>
              Cancel
            </button>
            <button
              className="btn danger"
              onClick={() => {
                const m = confirm!;
                setConfirm(null);
                void run(() => api.deleteContent(server.id, m.file));
              }}
            >
              Delete
            </button>
          </>
        }
      >
        <p>
          <span className="mono">{confirm?.file}</span> will be removed from the server. To keep it but stop it loading, turn it off instead.
        </p>
      </Modal>
    </div>
  );
}
