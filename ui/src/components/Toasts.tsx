import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";

type Kind = "info" | "success" | "error";
interface Toast {
  id: number;
  kind: Kind;
  text: string;
}

const Ctx = createContext<(text: string, kind?: Kind) => void>(() => {});

export const useToast = () => useContext(Ctx);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const next = useRef(1);

  const dismiss = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);
  const push = useCallback(
    (text: string, kind: Kind = "info") => {
      const id = next.current++;
      setToasts((t) => [...t.slice(-3), { id, kind, text }]);
      setTimeout(() => dismiss(id), kind === "error" ? 8000 : 3500);
    },
    [dismiss],
  );

  return (
    <Ctx.Provider value={push}>
      {children}
      {toasts.map((t, i) => (
        <ToastItem key={t.id} toast={t} index={toasts.length - 1 - i} onClose={() => dismiss(t.id)} />
      ))}
    </Ctx.Provider>
  );
}

/** popover="manual" puts each toast in the top layer, so it shows above open modals too. */
function ToastItem({ toast, index, onClose }: { toast: Toast; index: number; onClose: () => void }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (el && "showPopover" in el && !el.matches(":popover-open")) el.showPopover();
  }, []);
  return (
    <div
      ref={ref}
      popover="manual"
      role={toast.kind === "error" ? "alert" : "status"}
      className={`toast ${toast.kind}`}
      style={{ bottom: `${16 + index * 64}px` }}
    >
      <span>{toast.text}</span>
      <button className="icon-btn" aria-label="Dismiss" onClick={onClose}>
        ✕
      </button>
    </div>
  );
}
