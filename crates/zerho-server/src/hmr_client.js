// zerho native HMR client & Shadow DOM Error Overlay (100% Zero-Node)

const hotModules = new Map();
const hotData = new Map();

export function createHotContext(ownerPath) {
  if (!hotData.has(ownerPath)) {
    hotData.set(ownerPath, {});
  }
  const existing = hotModules.get(ownerPath);
  if (existing) {
    existing.callbacks = [];
    existing.disposeCallbacks = [];
  } else {
    hotModules.set(ownerPath, {
      id: ownerPath,
      callbacks: [],
      disposeCallbacks: [],
    });
  }

  return {
    get data() {
      return hotData.get(ownerPath);
    },
    accept(cb) {
      const mod = hotModules.get(ownerPath);
      if (mod) {
        mod.callbacks.push(typeof cb === "function" ? cb : () => {});
      }
    },
    dispose(cb) {
      const mod = hotModules.get(ownerPath);
      if (mod && typeof cb === "function") {
        mod.disposeCallbacks.push(cb);
      }
    },
    invalidate() {
      location.reload();
    },
  };
}

export function clearErrorOverlay() {
  document.querySelectorAll("zerho-error-overlay").forEach((el) => el.remove());
}

export function showErrorOverlay(file, message) {
  clearErrorOverlay();
  const host = document.createElement("zerho-error-overlay");
  const shadow = host.attachShadow({ mode: "open" });

  const safeFile = escapeHtml(file || "unknown file");
  const safeMessage = escapeHtml(message || "Unknown transform error");

  shadow.innerHTML = `
    <style>
      :host {
        position: fixed;
        inset: 0;
        z-index: 2147483647;
        background: rgba(9, 13, 20, 0.88);
        backdrop-filter: blur(8px);
        display: grid;
        place-items: center;
        padding: 1.5rem;
        font-family: "Red Hat Text", system-ui, sans-serif;
      }
      .box {
        width: min(760px, 95vw);
        max-height: 85vh;
        overflow: auto;
        background: #111827;
        border: 1px solid #ef4444;
        border-top: 4px solid #ef4444;
        border-radius: 14px;
        color: #f1f5f9;
        box-shadow: 0 24px 64px rgba(0, 0, 0, 0.6);
      }
      .head {
        display: flex;
        justify-content: space-between;
        align-items: center;
        padding: 1rem 1.25rem;
        border-bottom: 1px solid #24324a;
        gap: 1rem;
      }
      .title {
        font-family: "Red Hat Display", sans-serif;
        font-size: 1.15rem;
        font-weight: 700;
        color: #f87171;
      }
      .file {
        font-family: "Red Hat Mono", monospace;
        font-size: 0.82rem;
        color: #fb923c;
        padding: 0.75rem 1.25rem 0;
      }
      pre {
        margin: 0.75rem 1.25rem 1.25rem;
        padding: 1rem;
        border-radius: 10px;
        background: #090d14;
        border: 1px solid #24324a;
        font-family: "Red Hat Mono", monospace;
        font-size: 0.85rem;
        line-height: 1.5;
        white-space: pre-wrap;
        word-break: break-word;
        color: #fca5a5;
      }
      button {
        cursor: pointer;
        border: 1px solid #334155;
        background: #1e293b;
        color: #f1f5f9;
        border-radius: 8px;
        padding: 0.35rem 0.75rem;
        font-family: "Red Hat Mono", monospace;
        font-size: 0.8rem;
      }
      .hint {
        padding: 0 1.25rem 1rem;
        font-size: 0.8rem;
        color: #94a3b8;
      }
    </style>
    <div class="box" role="alertdialog" aria-label="zerho compile error">
      <div class="head">
        <div class="title">[zerho] Transform / Build Error</div>
        <button id="close-btn">Dismiss (Esc)</button>
      </div>
      <div class="file">${safeFile}</div>
      <pre>${safeMessage}</pre>
      <div class="hint">Fix the file and save — zerho will clear this overlay automatically.</div>
    </div>
  `;

  shadow.getElementById("close-btn")?.addEventListener("click", clearErrorOverlay);
  document.body.appendChild(host);
}

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    clearErrorOverlay();
  }
});

function escapeHtml(text) {
  return String(text)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;");
}

async function applyJsUpdate(path, timestamp) {
  const mod = hotModules.get(path);
  if (!mod || mod.callbacks.length === 0) {
    console.info(`[zerho] no HMR boundary for ${path}, reloading...`);
    location.reload();
    return;
  }

  for (const dispose of mod.disposeCallbacks) {
    try {
      await dispose(hotData.get(path));
    } catch (err) {
      console.error(`[zerho] HMR dispose error in ${path}:`, err);
    }
  }

  const callbacks = [...mod.callbacks];
  const updatedModule = await import(`${path}?t=${timestamp || Date.now()}`);
  clearErrorOverlay();

  for (const cb of callbacks) {
    try {
      cb(updatedModule);
    } catch (err) {
      showErrorOverlay(path, err?.stack || String(err));
      return;
    }
  }
  console.info(`[zerho] hot-updated module: ${path}`);
}

const protocol = location.protocol === "https:" ? "wss:" : "ws:";
const ws = new WebSocket(`${protocol}//${location.host}/@zerho/hmr`);

ws.addEventListener("open", () => {
  console.info("[zerho] HMR connected");
});

ws.addEventListener("message", async (event) => {
  try {
    const payload = JSON.parse(event.data);
    if (payload.type === "error") {
      showErrorOverlay(payload.file, payload.message);
    } else if (payload.type === "css-update" && payload.path) {
      clearErrorOverlay();
      await import(`${payload.path}?t=${payload.timestamp || Date.now()}`);
      console.info(`[zerho] hot-updated CSS: ${payload.path}`);
    } else if (payload.type === "js-update" && payload.path) {
      await applyJsUpdate(payload.path, payload.timestamp);
    } else if (payload.type === "full-reload") {
      clearErrorOverlay();
      console.info(`[zerho] reloading: ${payload.path ?? "file changed"}`);
      location.reload();
    }
  } catch (err) {
    console.error("[zerho] HMR error:", err);
  }
});

ws.addEventListener("close", () => {
  console.warn("[zerho] HMR disconnected");
});
