import type { GameInfo } from "../api";
import { formatMb } from "../util";

interface Props {
  game: GameInfo;
  /** MB; `null` means no limit (only for games with `memoryLimit`). */
  value: number | null;
  onChange: (mb: number | null) => void;
  max: number;
  totalMemoryMb?: number | null;
  /** Show the sizing hint under the slider. */
  hint?: boolean;
}

/** Starting point when someone turns a limit on: Palworld is far hungrier than the rest. */
export function suggestedLimit(game: GameInfo, max: number): number {
  return Math.min(max, game.id === "palworld" ? 8192 : 4096);
}

/**
 * Minecraft Java: the memory the server gets (Java heap).
 * Other games: an optional cap that Hearth enforces, off by default.
 */
export function MemoryField({ game, value, onChange, max, totalMemoryMb, hint = true }: Props) {
  const pc = totalMemoryMb ? `This PC has ${formatMb(totalMemoryMb)}.` : "";
  if (game.configurableMemory) {
    const mb = value ?? 2048;
    return (
      <label className="field">
        <span>
          Memory <strong className="mono">{formatMb(mb)}</strong>
        </span>
        <input type="range" min={1024} max={Math.max(max, mb)} step={512} value={mb} onChange={(e) => onChange(+e.target.value)} />
        {hint && <small className="hint">2–4 GB suits a handful of friends. {pc}</small>}
      </label>
    );
  }
  if (!game.memoryLimit) return null;
  const on = value !== null;
  return (
    <div className="field">
      <label className="check">
        <input type="checkbox" checked={on} onChange={(e) => onChange(e.target.checked ? suggestedLimit(game, max) : null)} />
        Limit memory {on && <strong className="mono">{formatMb(value)}</strong>}
      </label>
      {on && (
        <input
          type="range"
          aria-label="Memory limit"
          min={1024}
          max={Math.max(max, value)}
          step={512}
          value={value}
          onChange={(e) => onChange(+e.target.value)}
        />
      )}
      {hint && (
        <small className="hint">
          {on
            ? `The server can't use more than this. If it needs more, it slows down or crashes, so leave some headroom. ${pc}`
            : `Off: the server uses what it needs. Turn on to keep it from crowding out your game. ${pc}`}
        </small>
      )}
    </div>
  );
}
