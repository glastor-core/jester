import "./dashboard.css";
import { createInitialState } from "./store";

const state = createInitialState();

function renderDashboard(root: HTMLElement): void {
  const cards = state.metrics
    .map(
      (m) => `<div class="metric">
        <div class="metric-val">${m.value}</div>
        <div class="metric-lbl">${m.label}</div>
      </div>`,
    )
    .join("");

  root.innerHTML = `<main class="shell">
    <header class="topbar">
      <div class="brand">
        <span class="material-symbols-outlined notranslate" translate="no" style="color:#f97316;">bolt</span>
        <span>zerho Dashboard</span>
      </div>
      <span class="badge">Zero-Node · v1.0.0</span>
    </header>
    <section class="metrics">${cards}</section>
    <button id="load-analytics" class="btn" type="button">
      <span class="material-symbols-outlined notranslate" translate="no">analytics</span>
      <span>Load Async Analytics Chunk</span>
    </button>
    <div id="analytics-slot"></div>
  </main>`;

  const btn = document.getElementById("load-analytics");
  const slot = document.getElementById("analytics-slot");
  btn?.addEventListener("click", async () => {
    const mod = await import("./AnalyticsPanel");
    if (slot) {
      slot.innerHTML = mod.default();
    }
  });
}

const rootEl = document.getElementById("root");
if (rootEl) {
  renderDashboard(rootEl);
}
