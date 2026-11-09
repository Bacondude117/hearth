import { useEffect, useMemo, useState } from "react";
import { api, type GameInfo, type GameVersion, type Server } from "../api";
import { errorText, formatMb } from "../util";
import { Modal } from "./Modal";

interface Props {
  open: boolean;
  games: GameInfo[];
  totalMemoryMb: number | null;
  onClose: () => void;
  onCreated: (s: Server) => void;
}

export function NewServerDialog({ open, games, totalMemoryMb, onClose, onCreated }: Props) {
  const [gameId, setGameId] = useState("minecraft-java");
  const game = games.find((g) => g.id === gameId) ?? games[0];
  const [flavor, setFlavor] = useState("paper");
  const [versions, setVersions] = useState<GameVersion[] | null>(null);
  const [versionsError, setVersionsError] = useState<string | null>(null);
  const [showPre, setShowPre] = useState(false);
  const [version, setVersion] = useState("");
  const [name, setName] = useState("");
  const [port, setPort] = useState("");
  const [memory, setMemory] = useState(2048);
  const [eula, setEula] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const maxMemory = useMemo(() => {
    const total = totalMemoryMb ?? 8192;
    // Leave room for Windows and the game client itself.
    return Math.max(1024, Math.min(32768, Math.floor((total - 2048) / 512) * 512));
  }, [totalMemoryMb]);

  // Reset when reopened.
  useEffect(() => {
    if (!open) return;
    setName("");
    setEula(false);
    setError(null);
    setBusy(false);
    setMemory(Math.min(4096, maxMemory));
  }, [open, maxMemory]);

  // Default flavor + port when the game changes.
  useEffect(() => {
    if (!open || !game) return;
    setFlavor(game.flavors[0].id);
    setPort("");
    api.suggestPort(game.id).then((p) => setPort(String(p)), () => setPort(String(game.defaultPort)));
  }, [open, game]);

  // Load versions for the chosen flavor.
  useEffect(() => {
    if (!open || !game) return;
    let stale = false;
    setVersions(null);
    setVersionsError(null);
    api.versions(game.id, flavor).then(
      (v) => {
        if (stale) return;
        setVersions(v);
        setVersion(v.find((x) => x.stable)?.id ?? v[0]?.id ?? "");
      },
      (e) => !stale && setVersionsError(errorText(e)),
    );
    return () => {
      stale = true;
    };
  }, [open, game, flavor]);

  if (!game) return null;
  const shown = (versions ?? []).filter((v) => showPre || v.stable || v.id === version);
  const portNum = Number(port);
  const portValid = Number.isInteger(portNum) && portNum >= 1024 && portNum <= 65535;
  const canCreate = !busy && name.trim().length > 0 && !!version && portValid && (!game.eula || eula);

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canCreate) return;
    setBusy(true);
    setError(null);
    try {
      const s = await api.createServer({
        name: name.trim(),
        game: game.id,
        flavor,
        version,
        port: portNum,
        memoryMb: game.configurableMemory ? memory : null,
        acceptEula: eula,
      });
      onCreated(s);
    } catch (err) {
      setError(errorText(err));
      setBusy(false);
    }
  };

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="New server"
      wide
      locked={busy}
      footer={
        <>
          <button className="btn ghost" type="button" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button className="btn primary" type="submit" form="new-server" disabled={!canCreate}>
            {busy ? "Creating…" : "Create & install"}
          </button>
        </>
      }
    >
      <form id="new-server" className="form" onSubmit={submit}>
        <fieldset className="field">
          <legend>Game</legend>
          <div className="choice-grid">
            {games.map((g) => (
              <label key={g.id} className={`choice ${g.id === gameId ? "selected" : ""}`}>
                <input type="radio" name="game" value={g.id} checked={g.id === gameId} onChange={() => setGameId(g.id)} />
                <span className={`game-icon ${g.id}`} aria-hidden />
                <span>
                  <strong>{g.name}</strong>
                  <small>{g.description}</small>
                </span>
              </label>
            ))}
          </div>
        </fieldset>

        {game.flavors.length > 1 && (
          <fieldset className="field">
            <legend>Server type</legend>
            <div className="choice-grid">
              {game.flavors.map((f) => (
                <label key={f.id} className={`choice compact ${f.id === flavor ? "selected" : ""}`}>
                  <input type="radio" name="flavor" value={f.id} checked={f.id === flavor} onChange={() => setFlavor(f.id)} />
                  <span>
                    <strong>{f.name}</strong>
                    <small>{f.description}</small>
                  </span>
                </label>
              ))}
            </div>
          </fieldset>
        )}

        <div className="row">
          <label className="field grow">
            <span>Name</span>
            <input
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g. Survival with friends"
              maxLength={48}
              required
              autoFocus
            />
          </label>
          <label className="field">
            <span>Version</span>
            {versionsError ? (
              <span className="inline-error">Couldn't load versions: {versionsError}</span>
            ) : (
              <select value={version} onChange={(e) => setVersion(e.target.value)} disabled={!versions}>
                {!versions && <option>Loading…</option>}
                {shown.map((v) => (
                  <option key={v.id} value={v.id}>
                    {v.label}
                  </option>
                ))}
              </select>
            )}
            {versions?.some((v) => !v.stable) && (
              <label className="check small">
                <input type="checkbox" checked={showPre} onChange={(e) => setShowPre(e.target.checked)} /> Show pre-releases
              </label>
            )}
          </label>
          <label className="field port">
            <span>Port</span>
            <input
              inputMode="numeric"
              value={port}
              onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))}
              aria-invalid={port !== "" && !portValid}
            />
            {port !== "" && !portValid && <small className="inline-error">1024 – 65535</small>}
          </label>
        </div>

        {game.configurableMemory && (
          <label className="field">
            <span>
              Memory <strong className="mono">{formatMb(memory)}</strong>
            </span>
            <input type="range" min={1024} max={maxMemory} step={512} value={memory} onChange={(e) => setMemory(+e.target.value)} />
            <small className="hint">
              2–4 GB suits a handful of friends. {totalMemoryMb ? `This PC has ${formatMb(totalMemoryMb)}.` : ""}
            </small>
          </label>
        )}

        {game.eula && (
          <div className="eula">
            <p>{game.eula.text}</p>
            <p className="links">
              {game.eula.links.map(([label, url]) => (
                <button key={url} type="button" className="link" onClick={() => api.openLink(url)}>
                  {label} ↗
                </button>
              ))}
            </p>
            <label className="check">
              <input type="checkbox" checked={eula} onChange={(e) => setEula(e.target.checked)} /> I have read and accept these terms
            </label>
          </div>
        )}

        {error && <p className="form-error" role="alert">{error}</p>}
      </form>
    </Modal>
  );
}
