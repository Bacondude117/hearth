import { useEffect, useState } from "react";
import { api, type Server } from "../api";

// Loaded icons, keyed by server id + version so a changed icon is fetched once.
const cache = new Map<string, Promise<string | null>>();

function load(id: string, version: number): Promise<string | null> {
  const key = `${id}:${version}`;
  let p = cache.get(key);
  if (!p) {
    p = api.serverIcon(id).catch(() => null);
    cache.set(key, p);
  }
  return p;
}

/** The server's custom icon if it has one, otherwise its game's default artwork. */
export function ServerIcon({ server, big }: { server: Server; big?: boolean }) {
  const [src, setSrc] = useState<string | null>(null);
  const version = server.iconVersion;

  useEffect(() => {
    let stale = false;
    if (version == null) {
      setSrc(null);
      return;
    }
    void load(server.id, version).then((s) => !stale && setSrc(s));
    return () => {
      stale = true;
    };
  }, [server.id, version]);

  const cls = `game-icon ${big ? "big " : ""}${server.game}`;
  return src ? <img className={cls} src={src} alt="" aria-hidden /> : <span className={cls} aria-hidden />;
}
