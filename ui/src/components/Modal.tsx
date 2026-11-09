import { useEffect, useId, useRef, type ReactNode } from "react";

interface Props {
  open: boolean;
  onClose: () => void;
  title: string;
  children: ReactNode;
  footer?: ReactNode;
  wide?: boolean;
  /** Prevent closing via backdrop/Esc (e.g. while a request is running). */
  locked?: boolean;
}

const supportsClosedBy = typeof HTMLDialogElement !== "undefined" && "closedBy" in HTMLDialogElement.prototype;

/** Native modal <dialog>: top layer, focus trap and Esc handling come from the browser. */
export function Modal({ open, onClose, title, children, footer, wide, locked }: Props) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) d.showModal();
    if (!open && d.open) d.close();
  }, [open]);

  // Light-dismiss fallback for engines without `closedby` (WebKitGTK on Linux dev builds).
  const onClick = (e: React.MouseEvent<HTMLDialogElement>) => {
    if (supportsClosedBy || locked || e.target !== ref.current) return;
    const r = ref.current.getBoundingClientRect();
    const inside = r.top <= e.clientY && e.clientY <= r.bottom && r.left <= e.clientX && e.clientX <= r.right;
    if (!inside) ref.current.close();
  };

  return (
    <dialog
      ref={ref}
      className={wide ? "modal wide" : "modal"}
      aria-labelledby={titleId}
      // React doesn't know this attribute yet; lowercase passes through to the DOM.
      {...{ closedby: locked ? "none" : "any" }}
      onClose={onClose}
      onCancel={(e) => locked && e.preventDefault()}
      onClick={onClick}
    >
      <header className="modal-head">
        <h2 id={titleId}>{title}</h2>
        <button className="icon-btn" aria-label="Close" onClick={() => !locked && onClose()} disabled={locked}>
          ✕
        </button>
      </header>
      <div className="modal-body">{children}</div>
      {footer && <footer className="modal-foot">{footer}</footer>}
    </dialog>
  );
}
