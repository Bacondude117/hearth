import { useEffect, useMemo, useState } from "react";
import { api, type GameInfo, type GameVersion, type PackInfo, type Server } from "../api";
import { errorText, formatMb, loaderName } from "../util";
import { Modal } from "./Modal";

interface Props {
  open: boolean;
  game: GameInfo | undefined;
  totalMemoryMb: number | null;
  onClose: () => void;
  onCreated: (s: Server) => void;
}

const LOADERS = ["fabric", "neoforge", "forge"];

type Step = { kind: "pick" } | { kind: "inspecting"; file: string } | { kind: "ready"; file: string; info: PackInfo };

export function ImportPackDialog({ open, game, totalMemoryMb, onClose, onCreated }: Props) {
  const [step, setStep] = useState<Step>({ kind: "pick" });
  const [error, setError] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [loader, setLoader] = useState<string>("");
  const [minecraft, setMinecraft] = useState("");
  const [versions, setVersions] = useState<GameVersion[] | null>(null);
  const [port, setPort] = useState("");
  const [memory, setMemory] = useState(6144);
  const [eula, setEula] = useState(false);
  const [busy, setBusy] = useState(false);

  const maxMemory = useMemo(() => {
    const total = totalMemoryMb ?? 8192;
    return Math.max(1024, Math.min(32768, Math.floor((total - 2048) / 512) * 512));
  }, [totalMemoryMb]);

  useEffect(() => {
    if (!open) return;
    setStep({ kind: "pick" });
    setError(null);
    setEula(false);
    setBusy(false);
    // Modpacks need more memory than vanilla.
    setMemory(Math.min(6144, maxMemory));
    api.suggestPort("minecraft-java").then((p) => setPort(String(p)), () => setPort("25565"));
  }, [open, maxMemory]);

  // Versions for the manual Minecraft-version picker.
  const info = step.kind === "ready" ? step.info : null;
  const askVersion = !!info && !info.minecraft;
  useEffect(() => {
    if (!askVersion || !loader) return;
    setVersions(null);
    api.versions("minecraft-java", loader).then(setVersions, () => setVersions([]));
  }, [askVersion, loader]);

  const pick = async () => {
    setError(null);
    try {
      const file = await api.pickPackFile();
      if (!file) return;
      setStep({ kind: "inspecting", file });
      const info = await api.inspectPack(file);
      setName(info.name.slice(0, 48));
      setLoader(info.loader ?? "");
      setMinecraft(info.minecraft ?? "");
      setStep({ kind: "ready", file, info });
    } catch (e) {
      setError(errorText(e));
      setStep({ kind: "pick" });
    }
  };

  const portNum = Number(port);
  const portValid = Number.isInteger(portNum) && portNum >= 1024 && portNum <= 65535;
  const canCreate = !!info && !busy && !!name.trim() && !!loader && !!minecraft && portValid && (!game?.eula || eula);

  const create = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!canCreate || step.kind !== "ready") return;
    setBusy(true);
    setError(null);
    try {
      const s = await api.importPack({
        file: step.file,
        name: name.trim(),
        port: portNum,
        memoryMb: memory,
        acceptEula: eula,
        minecraft: info!.minecraft ? null : minecraft,
        loader: info!.loader ? null : loader,
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
      title="Import modpack"
      wide
      locked={busy}
      footer={
        <>
          <button className="btn ghost" type="button" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          {step.kind === "ready" && (
            <button className="btn primary" type="submit" form="import-pack" disabled={!canCreate}>
              {busy ? "Importing…" : "Create & install"}
            </button>
          )}
        </>
      }
    >
      {step.kind === "pick" && (
        <div className="pack-pick">
          <p>
            Choose a <strong>Modrinth .mrpack</strong> file, or a pack's <strong>server pack .zip</strong> (look for a
            "Server Pack" or "Server Files" download on the pack's page).
          </p>
          <button className="btn primary big" onClick={pick}>
            Choose file…
          </button>
          <p className="muted small">Fabric, Forge and NeoForge packs are supported.</p>
        </div>
      )}

      {step.kind === "inspecting" && (
        <div className="pack-pick">
          <div className="spinner" />
          <p className="muted">Reading {step.file.split(/[\\/]/).pop()}…</p>
        </div>
      )}

      {step.kind === "ready" && info && (
        <form id="import-pack" className="form" onSubmit={create}>
          <div className="pack-summary">
            <div className="pack-title">
              <strong>{info.name}</strong>
              {info.packVersion && <span className="muted"> {info.packVersion}</span>}
              <span className="tag">{info.kind === "mrpack" ? "Modrinth pack" : "Server pack"}</span>
            </div>
            <dl>
              <dt>Minecraft</dt>
              <dd>{info.minecraft ?? <em className="inline-error">not detected</em>}</dd>
              <dt>Loader</dt>
              <dd>
                {info.loader ? `${loaderName(info.loader)}${info.loaderVersion ? ` ${info.loaderVersion}` : ""}` : <em className="inline-error">not detected</em>}
              </dd>
              <dt>Mods</dt>
              <dd>{info.modCount}</dd>
            </dl>
            {info.notes.map((n) => (
              <p key={n} className="muted small">
                {n}
              </p>
            ))}
            {info.warnings.map((w) => (
              <p key={w} className="inline-error">
                ⚠ {w}
              </p>
            ))}
            <button type="button" className="link small" onClick={pick}>
              Choose a different file
            </button>
          </div>

          {(!info.loader || !info.minecraft) && (
            <div className="row">
              <p className="muted small grow-text">Hearth couldn't detect everything. Check the pack's page for these:</p>
              {!info.loader && (
                <label className="field">
                  <span>Loader</span>
                  <select value={loader} onChange={(e) => setLoader(e.target.value)} required>
                    <option value="">Choose…</option>
                    {LOADERS.map((l) => (
                      <option key={l} value={l}>
                        {loaderName(l)}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              {!info.minecraft && (
                <label className="field">
                  <span>Minecraft version</span>
                  <select value={minecraft} onChange={(e) => setMinecraft(e.target.value)} disabled={!loader || !versions} required>
                    <option value="">{loader ? (versions ? "Choose…" : "Loading…") : "Pick a loader first"}</option>
                    {versions?.map((v) => (
                      <option key={v.id} value={v.id}>
                        {v.label}
                      </option>
                    ))}
                  </select>
                </label>
              )}
            </div>
          )}

          <div className="row">
            <label className="field grow">
              <span>Name</span>
              <input value={name} onChange={(e) => setName(e.target.value)} maxLength={48} required />
            </label>
            <label className="field port">
              <span>Port</span>
              <input inputMode="numeric" value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, ""))} aria-invalid={port !== "" && !portValid} />
            </label>
          </div>

          <label className="field">
            <span>
              Memory <strong className="mono">{formatMb(memory)}</strong>
            </span>
            <input type="range" min={2048} max={maxMemory} step={512} value={memory} onChange={(e) => setMemory(+e.target.value)} />
            <small className="hint">Modpacks usually need 6–8 GB. Check the pack's page for its recommendation.</small>
          </label>

          {game?.eula && (
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
        </form>
      )}

      {error && (
        <p className="form-error" role="alert">
          {error}
        </p>
      )}
    </Modal>
  );
}
