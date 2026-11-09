import { useEffect, useMemo, useState } from "react";
import { api, isActive, type AppInfo, type GameInfo, type GameVersion, type Server } from "../api";
import { errorText, formatMb } from "../util";
import { Modal } from "./Modal";
import { useToast } from "./Toasts";

type Field =
  | { key: string; label: string; kind: "text"; hint?: string }
  | { key: string; label: string; kind: "number"; min: number; max: number; hint?: string }
  | { key: string; label: string; kind: "bool"; hint?: string }
  | { key: string; label: string; kind: "select"; options: string[]; hint?: string };

const JAVA_FIELDS: Field[] = [
  { key: "motd", label: "Message of the day", kind: "text", hint: "Shown in the multiplayer server list." },
  { key: "max-players", label: "Max players", kind: "number", min: 1, max: 1000 },
  { key: "gamemode", label: "Game mode", kind: "select", options: ["survival", "creative", "adventure", "spectator"] },
  { key: "difficulty", label: "Difficulty", kind: "select", options: ["peaceful", "easy", "normal", "hard"] },
  { key: "pvp", label: "Player vs player", kind: "bool" },
  { key: "white-list", label: "Whitelist only", kind: "bool", hint: "Add players with: whitelist add Name" },
  { key: "online-mode", label: "Require Minecraft accounts", kind: "bool", hint: "Turn off only if you know why (cracked clients)." },
  { key: "allow-flight", label: "Allow flight", kind: "bool" },
  { key: "view-distance", label: "View distance", kind: "number", min: 3, max: 32 },
  { key: "server-port", label: "Port", kind: "number", min: 1024, max: 65535 },
  { key: "level-seed", label: "World seed", kind: "text", hint: "Only used when a new world is generated." },
];

const BEDROCK_FIELDS: Field[] = [
  { key: "server-name", label: "Server name", kind: "text" },
  { key: "max-players", label: "Max players", kind: "number", min: 1, max: 1000 },
  { key: "gamemode", label: "Game mode", kind: "select", options: ["survival", "creative", "adventure"] },
  { key: "difficulty", label: "Difficulty", kind: "select", options: ["peaceful", "easy", "normal", "hard"] },
  { key: "allow-cheats", label: "Allow cheats", kind: "bool" },
  { key: "allow-list", label: "Allow list only", kind: "bool", hint: "Add players with: allowlist add Name" },
  { key: "online-mode", label: "Require Xbox sign-in", kind: "bool" },
  { key: "view-distance", label: "View distance", kind: "number", min: 5, max: 96 },
  { key: "server-port", label: "Port", kind: "number", min: 1024, max: 65535 },
  { key: "level-seed", label: "World seed", kind: "text", hint: "Only used when a new world is generated." },
];

interface Props {
  server: Server;
  game: GameInfo | undefined;
  info: AppInfo;
  onDeleted: () => void;
}

export function SettingsTab({ server, game, info, onDeleted }: Props) {
  const toast = useToast();
  const [props, setProps] = useState<[string, string][] | null>(null);
  const [edits, setEdits] = useState<Record<string, string>>({});
  const [showAll, setShowAll] = useState(false);
  const [saving, setSaving] = useState(false);
  const [name, setName] = useState(server.name);
  const [memory, setMemory] = useState(server.memoryMb ?? 2048);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [removeTunnel, setRemoveTunnel] = useState(true);
  const [updateOpen, setUpdateOpen] = useState(false);
  const installed = server.installState.state === "installed";
  const active = isActive(server.status);

  const load = () =>
    api.getProperties(server.id).then(
      (p) => {
        setProps(p);
        setEdits({});
      },
      (e) => toast(errorText(e), "error"),
    );

  useEffect(() => {
    setName(server.name);
    setMemory(server.memoryMb ?? 2048);
    setProps(null);
    if (installed) void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [server.id, installed]);

  const values = useMemo(() => new Map(props ?? []), [props]);
  const val = (k: string) => edits[k] ?? values.get(k) ?? "";
  const set = (k: string, v: string) => setEdits((e) => ({ ...e, [k]: v }));
  const fields = (server.game === "minecraft-bedrock" ? BEDROCK_FIELDS : JAVA_FIELDS).filter((f) => values.has(f.key) || f.key === "server-port");
  const dirty = Object.keys(edits).some((k) => edits[k] !== values.get(k));
  const metaDirty = name.trim() !== server.name || (game?.configurableMemory && memory !== server.memoryMb);
  const maxMemory = Math.max(1024, Math.min(32768, Math.floor(((info.totalMemoryMb ?? 8192) - 2048) / 512) * 512));

  const save = async () => {
    setSaving(true);
    try {
      if (metaDirty) {
        await api.updateServer(server.id, name.trim() !== server.name ? name.trim() : null, game?.configurableMemory && memory !== server.memoryMb ? memory : null);
      }
      if (dirty) {
        const changed = Object.fromEntries(Object.entries(edits).filter(([k, v]) => v !== values.get(k)));
        await api.setProperties(server.id, changed);
        await load();
      }
      toast(active ? "Saved. Restart the server to apply." : "Saved", "success");
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setSaving(false);
    }
  };

  const doDelete = async () => {
    try {
      await api.deleteServer(server.id, removeTunnel);
      setConfirmDelete(false);
      toast(`Deleted ${server.name}`, "success");
      onDeleted();
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  return (
    <div className="settings">
      <section className="card">
        <h3>General</h3>
        <div className="grid2">
          <label className="field">
            <span>Name in Hearth</span>
            <input value={name} maxLength={48} onChange={(e) => setName(e.target.value)} />
          </label>
          {game?.configurableMemory && (
            <label className="field">
              <span>
                Memory <strong className="mono">{formatMb(memory)}</strong>
              </span>
              <input type="range" min={1024} max={Math.max(maxMemory, memory)} step={512} value={memory} onChange={(e) => setMemory(+e.target.value)} />
            </label>
          )}
        </div>
      </section>

      {installed && (
        <section className="card">
          <h3>Game settings</h3>
          {!props ? (
            <p className="muted">Loading…</p>
          ) : props.length === 0 ? (
            <p className="muted">Settings appear after the server has started once.</p>
          ) : (
            <div className="grid2">
              {fields.map((f) => (
                <FieldInput key={f.key} field={f} value={f.key === "server-port" ? val(f.key) || String(server.port) : val(f.key)} onChange={(v) => set(f.key, v)} />
              ))}
            </div>
          )}
          {props && props.length > 0 && (
            <details className="all-props" open={showAll} onToggle={(e) => setShowAll((e.target as HTMLDetailsElement).open)}>
              <summary>All settings ({props.length})</summary>
              {showAll && (
                <table>
                  <tbody>
                    {props.map(([k]) => (
                      <tr key={k}>
                        <th className="mono">{k}</th>
                        <td>
                          <input value={val(k)} onChange={(e) => set(k, e.target.value)} aria-label={k} />
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
            </details>
          )}
        </section>
      )}

      <div className="save-bar">
        <button className="btn primary" disabled={saving || (!dirty && !metaDirty)} onClick={save}>
          {saving ? "Saving…" : "Save changes"}
        </button>
        {(dirty || metaDirty) && (
          <button
            className="btn ghost"
            onClick={() => {
              setEdits({});
              setName(server.name);
              setMemory(server.memoryMb ?? 2048);
            }}
          >
            Discard
          </button>
        )}
      </div>

      <section className="card">
        <h3>Maintenance</h3>
        <div className="maint">
          <div>
            <strong>{server.game === "minecraft-bedrock" ? "Update to the latest Bedrock server" : "Change version"}</strong>
            <p className="muted">
              {server.pack
                ? `This server came from the ${server.pack.name} modpack, so its Minecraft version is fixed. Changing it would break the mods.`
                : server.game === "minecraft-bedrock"
                ? "Bedrock players can only join a server on the same version as their game. Worlds and settings are kept."
                : "Downloads a different server version. Worlds and settings are kept. Back up first: worlds can't be opened by older versions."}
            </p>
          </div>
          <button className="btn" disabled={active || !installed || !!server.pack} onClick={() => setUpdateOpen(true)}>
            {server.game === "minecraft-bedrock" ? "Update" : "Change…"}
          </button>
        </div>
        <div className="maint">
          <div>
            <strong>Delete server</strong>
            <p className="muted">Removes the server and its worlds from this PC. This can't be undone.</p>
          </div>
          <button className="btn danger" disabled={active} onClick={() => setConfirmDelete(true)}>
            Delete…
          </button>
        </div>
      </section>

      <Modal
        open={confirmDelete}
        onClose={() => setConfirmDelete(false)}
        title={`Delete ${server.name}?`}
        footer={
          <>
            <button className="btn ghost" onClick={() => setConfirmDelete(false)}>
              Cancel
            </button>
            <button className="btn danger" onClick={doDelete}>
              Delete forever
            </button>
          </>
        }
      >
        <p>The server folder, including all worlds, will be permanently deleted.</p>
        {server.playitTunnelId && (
          <label className="check">
            <input type="checkbox" checked={removeTunnel} onChange={(e) => setRemoveTunnel(e.target.checked)} /> Also remove its playit.gg tunnel
          </label>
        )}
      </Modal>

      <UpdateDialog open={updateOpen} server={server} onClose={() => setUpdateOpen(false)} />
    </div>
  );
}

function FieldInput({ field, value, onChange }: { field: Field; value: string; onChange: (v: string) => void }) {
  const id = `f-${field.key}`;
  return (
    <label className={field.kind === "bool" ? "field toggle-field" : "field"} htmlFor={id}>
      <span>{field.label}</span>
      {field.kind === "bool" ? (
        <input id={id} type="checkbox" role="switch" className="switch" checked={value === "true"} onChange={(e) => onChange(String(e.target.checked))} />
      ) : field.kind === "select" ? (
        <select id={id} value={value} onChange={(e) => onChange(e.target.value)}>
          {!field.options.includes(value) && <option value={value}>{value || "(default)"}</option>}
          {field.options.map((o) => (
            <option key={o} value={o}>
              {o[0].toUpperCase() + o.slice(1)}
            </option>
          ))}
        </select>
      ) : field.kind === "number" ? (
        <input id={id} type="number" min={field.min} max={field.max} value={value} onChange={(e) => onChange(e.target.value)} />
      ) : (
        <input id={id} value={value} onChange={(e) => onChange(e.target.value)} />
      )}
      {field.hint && <small className="hint">{field.hint}</small>}
    </label>
  );
}

function UpdateDialog({ open, server, onClose }: { open: boolean; server: Server; onClose: () => void }) {
  const toast = useToast();
  const [versions, setVersions] = useState<GameVersion[] | null>(null);
  const [version, setVersion] = useState(server.version);
  const [busy, setBusy] = useState(false);
  const bedrock = server.game === "minecraft-bedrock";

  useEffect(() => {
    if (!open) return;
    setVersion(server.version);
    setVersions(null);
    api.versions(server.game, server.flavor).then(setVersions, (e) => toast(errorText(e), "error"));
  }, [open, server.game, server.flavor, server.version, toast]);

  const go = async () => {
    setBusy(true);
    try {
      await api.reinstallServer(server.id, bedrock ? null : version);
      toast("Updating… progress is shown on the Console tab.", "info");
      onClose();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={bedrock ? "Update Bedrock server" : "Change Minecraft version"}
      locked={busy}
      footer={
        <>
          <button className="btn ghost" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button className="btn primary" onClick={go} disabled={busy || !versions}>
            {busy ? "Starting…" : bedrock ? "Update now" : "Install this version"}
          </button>
        </>
      }
    >
      {bedrock ? (
        <p>
          Installed: <strong>{server.installedBuild ?? "unknown"}</strong>
          <br />
          Available: <strong>{versions?.[0]?.label ?? "checking…"}</strong>
        </p>
      ) : (
        <label className="field">
          <span>Version</span>
          <select value={version} onChange={(e) => setVersion(e.target.value)} disabled={!versions}>
            {(versions ?? [{ id: server.version, label: server.version, stable: true }]).map((v) => (
              <option key={v.id} value={v.id}>
                {v.label}
                {v.id === server.version ? " (current)" : ""}
              </option>
            ))}
          </select>
          <small className="hint">Moving to an older version can corrupt your world. Copy the server folder first.</small>
        </label>
      )}
    </Modal>
  );
}
