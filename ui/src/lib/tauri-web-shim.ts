// The web build runs the Svelte bundle outside a Tauri webview, where
// `window.__TAURI_INTERNALS__` does not exist and every `invoke()` rejects with
// a raw `TypeError`. This module installs the surface Tauri's own
// `@tauri-apps/api/mocks` installs, but wired to `omacal-server`'s HTTP routes
// rather than a mock callback: `invoke` becomes a POST to `/api/<cmd>`, and
// `listen` is intercepted locally, fed by the server's `/ws` channel.
//
// It is imported only when the build sets `VITE_TARGET=web`:
//
//   VITE_TARGET=web npm --prefix ui run build
//
// and statically drops out of the desktop build, where the flag is unset.

type InvokeArgs = Record<string, unknown> | number[] | ArrayBuffer | Uint8Array;
type UnknownCallback = (response: unknown) => void;

type TauriInternals = {
  invoke: (cmd: string, args?: InvokeArgs, options?: unknown) => Promise<unknown>;
  transformCallback: (callback?: UnknownCallback, once?: boolean) => number;
  unregisterCallback: (id: number) => void;
  runCallback: (id: number, data: unknown) => void;
  callbacks: Map<number, UnknownCallback>;
};

type EventPluginInternals = {
  unregisterListener: (event: string, eventId: number) => void;
};

const internals = globalThis as unknown as {
  __TAURI_INTERNALS__?: TauriInternals;
  __TAURI_EVENT_PLUGIN_INTERNALS__?: EventPluginInternals;
};

const callbacks = new Map<number, UnknownCallback>();
let nextCallbackId = 1;

const listeners = new Map<string, number[]>();

function transformCallback(callback?: UnknownCallback, once = false): number {
  const id = nextCallbackId++;
  callbacks.set(id, (data) => {
    if (once) callbacks.delete(id);
    callback?.(data);
  });
  return id;
}

function unregisterCallback(id: number): void {
  callbacks.delete(id);
}

function runCallback(id: number, data: unknown): void {
  callbacks.get(id)?.(data);
}

function removeListener(event: string, eventId: number): void {
  const registered = listeners.get(event);
  if (!registered) return;
  const index = registered.indexOf(eventId);
  if (index !== -1) registered.splice(index, 1);
}

/**
 * The server's event channel. `listen` is intercepted locally, so the events
 * the desktop would receive through Tauri arrive here as
 * `{ event, payload }` frames; each is dispatched into the registered
 * callbacks. Reconnects on close, because the page outlives a server restart.
 */
function connectEvents(): void {
  const protocol = location.protocol === "https:" ? "wss:" : "ws:";
  const socket = new WebSocket(`${protocol}//${location.host}/ws`);

  socket.onmessage = (message) => {
    let frame: { event?: string; payload?: unknown };
    try {
      frame = JSON.parse(String(message.data));
    } catch {
      return;
    }
    if (typeof frame.event !== "string") return;
    for (const id of listeners.get(frame.event) ?? []) {
      runCallback(id, { event: frame.event, id, payload: frame.payload });
    }
  };

  socket.onclose = () => {
    // A dropped connection is normal (server restart, sleep). Retry, so a page
    // left open keeps receiving syncs.
    setTimeout(connectEvents, 1000);
  };
  socket.onerror = () => {
    socket.close();
  };
}

async function invoke(cmd: string, args?: InvokeArgs): Promise<unknown> {
  if (cmd === "plugin:event|listen") {
    const { event, handler } = (args ?? {}) as { event: string; handler: number };
    listeners.set(event, [...(listeners.get(event) ?? []), handler]);
    return handler;
  }
  if (cmd === "plugin:event|unlisten") {
    const { event, eventId } = (args ?? {}) as { event: string; eventId: number };
    removeListener(event, eventId);
    return null;
  }
  if (cmd === "plugin:event|emit") {
    const { event, payload } = (args ?? {}) as { event: string; payload: unknown };
    for (const id of listeners.get(event) ?? []) {
      runCallback(id, { event, id, payload });
    }
    return null;
  }

  const response = await fetch(`/api/${cmd}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(args ?? {}),
  });
  if (!response.ok) {
    // Tauri rejects a command's `Err` with the Rust error's Display; the server
    // sends that string as the body, so existing `.catch` paths are unchanged.
    throw await response.text();
  }
  return response.json();
}

export function installWebShim(): void {
  internals.__TAURI_INTERNALS__ = {
    invoke,
    transformCallback,
    unregisterCallback,
    runCallback,
    callbacks,
  };
  internals.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener(event, eventId) {
      unregisterCallback(eventId);
      removeListener(event, eventId);
    },
  };

  connectEvents();
}

installWebShim();
