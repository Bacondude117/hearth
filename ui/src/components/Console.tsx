import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { api, onEvent, type LogLine } from "../api";

const MAX_LINES = 2000;

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
    setLines([]);
    stick.current = true;

    // Subscribe first so nothing is lost between the backlog fetch and live events.
    const off = onEvent((e) => {
      if (e.type !== "serverLog" || e.instanceId !== consoleId) return;
      if (lastSeq === 0) {
        pending.push(e.line);
        return;
      }
      if (e.line.seq <= lastSeq) return;
      lastSeq = e.line.seq;
      setLines((l) => {
        const next = l.length >= MAX_LINES ? l.slice(-MAX_LINES + 1) : l.slice();
        next.push(e.line);
        return next;
      });
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
    };
  }, [consoleId, load]);

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
        {lines.map((l) => (
          <div key={l.seq} className={`ln ${l.stream} ${lineTone(l.text)}`}>
            {l.text || " "}
          </div>
        ))}
      </div>
      {onCommand && (
        <form className="console-input" onSubmit={submit}>
          <span aria-hidden>›</span>
          <input
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={onKey}
            placeholder={canSend ? (placeholder ?? "Type a command, e.g. say hello or op YourName") : "Start the server to send commands"}
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

function lineTone(t: string): string {
  if (/\b(ERROR|SEVERE|FATAL)\b|Exception/.test(t)) return "err";
  if (/\bWARN(ING)?\b/.test(t)) return "warn";
  if (/joined the game|Player connected/.test(t)) return "join";
  return "";
}

/** Convenience loader for a server console. */
export const serverConsoleLoader = (id: string) => () => api.consoleLines(id);
