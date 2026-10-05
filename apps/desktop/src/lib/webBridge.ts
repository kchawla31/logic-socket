// Dev-only shim (VITE_WEB_BRIDGE=1): lets the UI run in a normal browser
// against `cargo run --example web_bridge`, which exposes the real Tauri
// commands over HTTP. Never included in the desktop build.
const BASE = 'http://127.0.0.1:1421';

type Cb = (payload: unknown) => void;
const callbacks = new Map<number, Cb>();
const listeners = new Map<string, Set<number>>();
let nextId = 1;

function ensureEvents() {
  const es = new EventSource(`${BASE}/events`);
  es.onmessage = m => {
    const { event, payload } = JSON.parse(m.data) as { event: string; payload: unknown };
    for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ event, id: 0, payload });
  };
}

export function installWebBridge() {
  ensureEvents();
  (window as unknown as { __TAURI_EVENT_PLUGIN_INTERNALS__: unknown }).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener(_event: string, id: number) {
      for (const s of listeners.values()) s.delete(id);
    },
  };
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
    transformCallback(cb: Cb) {
      const id = nextId++;
      callbacks.set(id, cb);
      return id;
    },
    unregisterCallback(id: number) {
      callbacks.delete(id);
    },
    async invoke(cmd: string, args: Record<string, unknown> = {}) {
      if (cmd === 'plugin:event|listen') {
        const ev = args.event as string;
        if (!listeners.has(ev)) listeners.set(ev, new Set());
        listeners.get(ev)!.add(args.handler as number);
        return args.handler;
      }
      if (cmd === 'plugin:event|unlisten') {
        for (const s of listeners.values()) s.delete(args.eventId as number);
        return null;
      }
      const res = await fetch(`${BASE}/invoke/${cmd}`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify(args),
      });
      const body = await res.json();
      if (!res.ok) throw body;
      return body;
    },
    convertFileSrc: (p: string) => p,
  };
}
