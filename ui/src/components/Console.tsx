import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api, onEvent, type LogLine } from "../api";

const MAX_LINES = 2000;
/** Lines are rendered in fixed chunks so only the newest chunk re-renders as lines arrive. */
const CHUNK = 100;

interface Props {
  consoleId: string;
  load: () => Promise<LogLine[]>;
  /** Omit for read-only consoles (playit agent log). */
  onCommand?: (line: string) => Promise<void>;
  canSend?: boolean;
  placeholder?: string;
}

/** Live log view. Sticks to the bottom unless the user scrolled up to read. */
export function Console({ consoleId, load, onCommand, canSend = true, placeholder }: Props) {
  const [lines, setLines] = useState<LogLine[]>([]);
  const [input, setInput] = useState("");
  const [history, setHistory] = useState<string[]>([]);
  const [histPos, setHistPos] = useState(-1);
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    let lastSeq = 0;
    let cancelled = false;
    const pending: LogLine[] = [];
    // Lines that arrive while the window is hidden are kept here and shown when it's visible
    // again, so a minimized Hearth does no rendering work.
    let hiddenBuffer: LogLine[] = [];
    setLines([]);
    stick.current = true;

    const append = (incoming: LogLine[]) => {
      const fresh = incoming.filter((l) => l.seq > lastSeq);
      if (!fresh.length) return;
      lastSeq = fresh[fresh.length - 1].seq;
      if (document.hidden) {
        hiddenBuffer.push(...fresh);
        if (hiddenBuffer.length > MAX_LINES) hiddenBuffer = hiddenBuffer.slice(-MAX_LINES);
        return;
      }
      setLines((l) => {
        const next = l.concat(fresh);
        return next.length > MAX_LINES ? next.slice(-MAX_LINES) : next;
      });
    };

    const onVisible = () => {
      if (document.hidden || !hiddenBuffer.length) return;
      const buffered = hiddenBuffer;
      hiddenBuffer = [];
      setLines((l) => l.concat(buffered).slice(-MAX_LINES));
    };
    document.addEventListener("visibilitychange", onVisible);

    // Subscribe first so nothing is lost between the backlog fetch and live events.
    const off = onEvent((e) => {
      if (e.type !== "serverLog" || e.instanceId !== consoleId) return;
      if (lastSeq === 0) {
        pending.push(...e.lines);
        return;
      }
      append(e.lines);
    });

    void load().then((backlog) => {
      if (cancelled) return;
      lastSeq = backlog.at(-1)?.seq ?? -1;
      const extra = pending.filter((l) => l.seq > lastSeq);
      if (extra.length) lastSeq = extra.at(-1)!.seq;
      setLines([...backlog, ...extra].slice(-MAX_LINES));
    });
    return () => {
      cancelled = true;
      off();
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [consoleId, load]);

  // Group by sequence number so a full chunk's array (and component) stays the same as new
  // lines arrive.
  const chunks = useMemo(() => {
    const out: { id: number; lines: LogLine[] }[] = [];
    for (const l of lines) {
      const id = Math.floor(l.seq / CHUNK);
      const last = out[out.length - 1];
      if (last && last.id === id) last.lines.push(l);
      else out.push({ id, lines: [l] });
    }
    return out;
  }, [lines]);
  const chunkCache = useRef(new Map<number, LogLine[]>());
  const stableChunks = chunks.map((c) => {
    const prev = chunkCache.current.get(c.id);
    const same = prev && prev.length === c.lines.length && prev[0]?.seq === c.lines[0]?.seq;
    if (same) return { id: c.id, lines: prev };
    chunkCache.current.set(c.id, c.lines);
    return c;
  });
  if (chunkCache.current.size > chunks.length + 4) {
    const live = new Set(chunks.map((c) => c.id));
    for (const k of chunkCache.current.keys()) if (!live.has(k)) chunkCache.current.delete(k);
  }

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines]);

  const onScroll = () => {
    const el = scroller.current;
    if (el) stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
  };

  const submit = async (e: React.FormEvent) => {
    e.preventDefault();
    const line = input.trim();
    if (!line || !onCommand) return;
    setInput("");
    setHistory((h) => [...h.filter((x) => x !== line), line].slice(-50));
    setHistPos(-1);
    stick.current = true;
    await onCommand(line);
  };

  const onKey = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    if (!history.length) return;
    const pos = e.key === "ArrowUp" ? (histPos < 0 ? history.length - 1 : Math.max(0, histPos - 1)) : histPos < 0 ? -1 : histPos + 1;
    if (pos < 0 || pos >= history.length) {
      setHistPos(-1);
      setInput("");
    } else {
      setHistPos(pos);
      setInput(history[pos]);
    }
  };

  return (
    <div className="console">
      <div className="console-log" ref={scroller} onScroll={onScroll} role="log" aria-live="off" tabIndex={0}>
        {lines.length === 0 && <div className="console-empty">Nothing here yet.</div>}
        {stableChunks.map((c, i) => (
          <Chunk key={c.id} lines={c.lines} deferred={i < stableChunks.length - 1} />
        ))}
      </div>
      {onCommand && (
        <form className="console-input" onSubmit={submit}>
          <span aria-hidden>›</span>
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={onKey}
            placeholder={canSend ? (placeholder ?? "Type a command") : "Start the server to send commands"}
            disabled={!canSend}
            aria-label="Console command"
            spellCheck={false}
            autoComplete="off"
          />
        </form>
      )}
    </div>
  );
}

/** A block of console lines. Older blocks skip layout/paint while scrolled out of view. */
const Chunk = memo(function Chunk({ lines, deferred }: { lines: LogLine[]; deferred: boolean }) {
  return (
    <div className={deferred ? "chunk deferred" : "chunk"}>
      {lines.map((l) => (
        <div key={l.seq} className={`ln ${l.stream} ${lineTone(l.text)}`}>
          {l.text || "\u00a0"}
        </div>
      ))}
    </div>
  );
});

function lineTone(t: string): string {
  if (/\b(ERROR|SEVERE|FATAL)\b|Exception/.test(t)) return "err";
  if (/\bWARN(ING)?\b/.test(t)) return "warn";
  if (/joined the game|Player connected/.test(t)) return "join";
  return "";
}

/** Convenience loader for a server console. */
export const serverConsoleLoader = (id: string) => () => api.consoleLines(id);
