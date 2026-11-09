import { useCallback, useEffect, useRef, useState } from "react";
import { api, onEvent, type PlayerAction, type PlayerEntry, type PlayersView, type Role, type Server } from "../api";
import { errorText } from "../util";
import { Modal } from "./Modal";
import { useToast } from "./Toasts";

const ROLE_LABEL: Record<Role, string> = {
  player: "Player",
  visitor: "Visitor",
  member: "Member",
  operator: "Operator",
};

const ROLE_HINT: Record<Role, string> = {
  player: "Can play, no commands",
  visitor: "Can look around but not build or break",
  member: "Can build, mine and fight",
  operator: "Full permissions: every command",
};

export function PlayersTab({ server }: { server: Server }) {
  const toast = useToast();
  const [view, setView] = useState<PlayersView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [newName, setNewName] = useState("");
  const [banTarget, setBanTarget] = useState<string | null>(null);
  const [banReason, setBanReason] = useState("");
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const load = useCallback(() => {
    api.listPlayers(server.id).then(
      (v) => {
        setView(v);
        setError(null);
      },
      (e) => setError(errorText(e)),
    );
  }, [server.id]);

  // A running server writes ops/whitelist files a moment after the command; re-read shortly after.
  const loadSoon = useCallback(() => {
    clearTimeout(refreshTimer.current);
    refreshTimer.current = setTimeout(load, 700);
  }, [load]);

  useEffect(() => {
    load();
    const off = onEvent((e) => {
      if ((e.type === "serverPlayers" || e.type === "serverStatus") && e.instanceId === server.id) loadSoon();
    });
    return () => {
      off();
      clearTimeout(refreshTimer.current);
    };
  }, [server.id, load, loadSoon]);

  const act = async (name: string, action: PlayerAction, done?: string) => {
    setBusy(name);
    try {
      await api.playerAction(server.id, name, action);
      if (done) toast(done, "success");
      load();
      loadSoon();
    } catch (e) {
      toast(errorText(e), "error");
    } finally {
      setBusy(null);
    }
  };

  if (server.installState.state !== "installed") {
    return <p className="muted pad">Players can be managed once the server is installed.</p>;
  }
  if (error) return <p className="form-error pad">{error}</p>;
  if (!view) return <p className="muted pad">Loading…</p>;

  const { caps } = view;
  const transitioning = server.status.state === "starting" || server.status.state === "stopping";
  const addPlayer = async (e: React.FormEvent) => {
    e.preventDefault();
    const name = newName.trim();
    if (!name) return;
    await act(name, { type: "whitelist", on: true }, `${name} added to the ${caps.whitelistLabel.toLowerCase()}`);
    setNewName("");
  };

  return (
    <div className="players">
      <div className="players-top">
        {caps.hasWhitelist ? (
        <form className="add-player" onSubmit={addPlayer}>
          <input
            value={newName}
            onChange={(e) => setNewName(e.target.value)}
            placeholder={server.game === "minecraft-bedrock" ? "Gamertag" : "Minecraft username"}
            aria-label="Player name"
            maxLength={20}
            spellCheck={false}
          />
          <button className="btn" type="submit" disabled={!newName.trim() || transitioning || busy !== null}>
            Add to {caps.whitelistLabel.toLowerCase()}
          </button>
        </form>
        ) : (
          <span />
        )}
        <p className="muted small apply-note">
          {view.running
            ? "Server is running: changes apply immediately."
            : "Server is stopped: changes are saved and apply on next start."}
        </p>
      </div>

      {caps.hasWhitelist && !view.whitelistEnabled && (
        <div className="banner info">
          The {caps.whitelistLabel.toLowerCase()} is <strong>off</strong>, so anyone with the address can join. Turn on{" "}
          <em>{caps.whitelistLabel === "Whitelist" ? "Whitelist only" : "Allow list only"}</em> in Settings to only let in
          the players ticked below.
        </div>
      )}

      {view.players.length === 0 ? (
        <div className="empty-players">
          <p>
            <strong>No players yet.</strong>
          </p>
          <p className="muted">Players appear here after they join{caps.hasWhitelist ? ", or add someone by name above" : ""}.</p>
        </div>
      ) : (
        <div className="table-wrap">
          <table className="players-table">
            <thead>
              <tr>
                <th>Player</th>
                {caps.roles.length > 0 && <th>Role</th>}
                {caps.hasWhitelist && <th>{caps.whitelistLabel}</th>}
                <th className="right">Actions</th>
              </tr>
            </thead>
            <tbody>
              {view.players.map((p) => (
                <PlayerRow
                  key={p.name}
                  p={p}
                  view={view}
                  disabled={transitioning || busy === p.name}
                  onAction={(a, done) => act(p.name, a, done)}
                  onBan={() => {
                    setBanReason("");
                    setBanTarget(p.name);
                  }}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}

      <Modal
        open={banTarget !== null}
        onClose={() => setBanTarget(null)}
        title={`Ban ${banTarget ?? ""}?`}
        footer={
          <>
            <button className="btn ghost" onClick={() => setBanTarget(null)}>
              Cancel
            </button>
            <button
              className="btn danger"
              onClick={async () => {
                const name = banTarget!;
                setBanTarget(null);
                await act(name, { type: "ban", reason: banReason.trim() || null }, `${name} is banned`);
              }}
            >
              Ban
            </button>
          </>
        }
      >
        <p>They'll be disconnected if online and can't rejoin until unbanned.</p>
        <label className="field">
          <span>Reason (optional, shown to the player)</span>
          <input value={banReason} onChange={(e) => setBanReason(e.target.value)} maxLength={200} />
        </label>
      </Modal>
    </div>
  );
}

interface RowProps {
  p: PlayerEntry;
  view: PlayersView;
  disabled: boolean;
  onAction: (a: PlayerAction, done?: string) => void;
  onBan: () => void;
}

function PlayerRow({ p, view, disabled, onAction, onBan }: RowProps) {
  const { caps } = view;
  const roleLocked = caps.roleNeedsId && !p.id;
  return (
    <tr className={p.banned ? "banned" : undefined}>
      <td>
        <div className="player-cell">
          <span className={`dot ${p.online ? "good" : "idle"}`} title={p.online ? "Online" : "Offline"} />
          <span className="player-name">{p.name}</span>
          {p.online && <span className="tag good">online</span>}
          {p.banned && (
            <span className="tag bad" title={p.banReason ?? undefined}>
              banned
            </span>
          )}
        </div>
      </td>
      {caps.roles.length > 0 && (
      <td>
        <select
          value={p.role}
          disabled={disabled || roleLocked}
          title={roleLocked ? "They need to join once before their role can be set" : ROLE_HINT[p.role]}
          aria-label={`Role for ${p.name}`}
          onChange={(e) => {
            const role = e.target.value as Role;
            onAction({ type: "setRole", role }, `${p.name} is now ${ROLE_LABEL[role].toLowerCase()}`);
          }}
        >
          {caps.roles.map((r) => (
            <option key={r} value={r} title={ROLE_HINT[r]}>
              {ROLE_LABEL[r]}
            </option>
          ))}
        </select>
      </td>
      )}
      {caps.hasWhitelist && (
      <td>
        <input
          type="checkbox"
          role="switch"
          className="switch"
          checked={p.whitelisted}
          disabled={disabled}
          aria-label={`${caps.whitelistLabel} ${p.name}`}
          onChange={(e) => onAction({ type: "whitelist", on: e.target.checked })}
        />
      </td>
      )}
      <td className="right">
        <div className="row-actions">
          {p.online && view.running && caps.gameModes.length > 0 && (
            <>
              <select
                value=""
                disabled={disabled}
                aria-label={`Game mode for ${p.name}`}
                onChange={(e) =>
                  e.target.value && onAction({ type: "gameMode", mode: e.target.value }, `${p.name} → ${e.target.value}`)
                }
              >
                <option value="">Game mode…</option>
                {caps.gameModes.map((m) => (
                  <option key={m} value={m}>
                    {m[0].toUpperCase() + m.slice(1)}
                  </option>
                ))}
              </select>
            </>
          )}
          {p.online && view.running && (
            <button className="btn small" disabled={disabled} onClick={() => onAction({ type: "kick", reason: null }, `Kicked ${p.name}`)}>
              Kick
            </button>
          )}
          {caps.canBan &&
            (p.banned ? (
              <button className="btn small" disabled={disabled} onClick={() => onAction({ type: "unban" }, `${p.name} is unbanned`)}>
                Unban
              </button>
            ) : (
              <button className="btn small danger" disabled={disabled} onClick={onBan}>
                Ban…
              </button>
            ))}
        </div>
      </td>
    </tr>
  );
}
