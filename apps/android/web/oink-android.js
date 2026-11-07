// Presents the Tauri IPC surface the shared interface (apps/oink) expects,
// backed by the Android app's `OinkAndroid` JavaScript interface. Loaded as a
// classic script before the interface's module scripts.
(function () {
  'use strict';
  const bridge = window.OinkAndroid;
  if (!bridge) return;
  // The interface is laid out for windows at least 820 px wide; narrower
  // screens (phones) get a wider viewport scaled to fit.
  const MIN_WIDTH = 840;
  const viewport = document.querySelector('meta[name="viewport"]');
  if (viewport && screen.width < MIN_WIDTH) viewport.content = `width=${MIN_WIDTH}`;
  const view = new URLSearchParams(location.search).get('view') || 'main';
  const pending = new Map();
  const callbacks = new Map();
  const listeners = new Map();
  let nextRequest = 1;
  let nextCallback = 1;
  let nextListener = 1;

  window.__oinkAndroid = {
    // `text` is `{"ok": value}` or `{"err": ApiError}` as JSON.
    reply(id, text) {
      const request = pending.get(id);
      if (!request) return;
      pending.delete(id);
      let value;
      try {
        value = JSON.parse(text);
      } catch (error) {
        request.reject(error);
        return;
      }
      if (value && Object.prototype.hasOwnProperty.call(value, 'err')) request.reject(value.err);
      else request.resolve(value ? value.ok : null);
    },
    // One ApiEvent, delivered as the desktop shell's `agent-event`.
    event(payload) {
      for (const [id, listener] of listeners) {
        if (listener.event !== 'agent-event') continue;
        const callback = callbacks.get(listener.handler);
        if (callback) callback({ event: 'agent-event', id, payload });
      }
    },
  };

  function native(command, args) {
    return new Promise((resolve, reject) => {
      const id = nextRequest++;
      pending.set(id, { resolve, reject });
      bridge.invoke(id, command, JSON.stringify(args || {}));
    });
  }

  // Window/webview plugin queries: one fixed, visible, focused window.
  function plugin(command) {
    const name = command.slice(command.indexOf('|') + 1);
    if (name === 'is_visible' || name === 'is_focused' || name === 'is_enabled') return true;
    if (name.startsWith('is_')) return false;
    if (name === 'scale_factor') return window.devicePixelRatio || 1;
    if (name === 'inner_size' || name === 'outer_size') {
      const ratio = window.devicePixelRatio || 1;
      return { width: Math.round(innerWidth * ratio), height: Math.round(innerHeight * ratio) };
    }
    if (name === 'inner_position' || name === 'outer_position') return { x: 0, y: 0 };
    if (name === 'theme') return matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    return null;
  }

  window.__TAURI_INTERNALS__ = {
    metadata: {
      currentWindow: { label: view },
      currentWebview: { windowLabel: view, label: view },
    },
    plugins: { path: { sep: '/', delimiter: ':' } },
    callbacks,
    transformCallback(callback, once) {
      const id = nextCallback++;
      callbacks.set(id, (data) => {
        if (once) callbacks.delete(id);
        if (callback) return callback(data);
      });
      return id;
    },
    unregisterCallback(id) {
      callbacks.delete(id);
    },
    runCallback(id, data) {
      const callback = callbacks.get(id);
      if (callback) callback(data);
    },
    convertFileSrc(path) {
      return path;
    },
    async invoke(command, args) {
      if (command === 'plugin:event|listen') {
        const id = nextListener++;
        listeners.set(id, { event: args.event, handler: args.handler });
        return id;
      }
      if (command === 'plugin:event|unlisten') {
        listeners.delete(args.eventId);
        return null;
      }
      if (command === 'plugin:event|emit' || command === 'plugin:event|emit_to') return null;
      if (command.startsWith('plugin:')) return plugin(command);
      return native(command, args);
    },
  };
  window.__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener(_event, id) {
      listeners.delete(id);
    },
  };
})();
