import { useState } from "react";
import { api, type PlayitStatus } from "../api";
import { errorText } from "../util";
import { useToast } from "./Toasts";

/** "Link your playit.gg account" flow, shared by the Share tab and the playit page. */
export function PlayitLink({ status }: { status: PlayitStatus }) {
  const toast = useToast();
  const [busy, setBusy] = useState(false);

  if (status.claim) {
    return (
      <div className="link-card waiting">
        <div className="spinner" aria-hidden />
        <div>
          <strong>{status.claim.state}…</strong>
          <p className="muted">
            A playit.gg page opened in your browser. Sign in or continue as a guest, then approve <em>Hearth</em>. This
            window updates by itself.
          </p>
          <div className="btn-row">
            <button className="btn small" onClick={() => api.openLink(status.claim!.url)}>
              Open the page again
            </button>
            <button className="btn ghost small" onClick={() => api.playitCancelLink()}>
              Cancel
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="link-card">
      <div>
        <strong>Connect playit.gg</strong>
        <p className="muted">
          playit.gg is a free service that gives your server a public address, so friends can join without router setup
          or port forwarding. You only do this once.
        </p>
      </div>
      <button
        className="btn primary"
        disabled={busy}
        onClick={async () => {
          setBusy(true);
          try {
            await api.playitLink();
          } catch (e) {
            toast(errorText(e), "error");
          } finally {
            setBusy(false);
          }
        }}
      >
        Link account
      </button>
    </div>
  );
}
