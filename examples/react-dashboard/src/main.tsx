import "./dashboard.css";
import { createInitialState, type PipelineStage } from "./store";

const state = createInitialState();
let activeTab = "overview";
let searchQuery = "";
let analyticsLoaded = false;

function renderStages(stages: readonly PipelineStage[]): string {
  const q = searchQuery.trim().toLowerCase();
  const filtered = q
    ? stages.filter(
        (s) => s.name.toLowerCase().includes(q) || s.crateName.toLowerCase().includes(q),
      )
    : stages;

  if (filtered.length === 0) {
    return `<p style="font-size:0.84rem;color:var(--text-dim);margin:8px 0;">No pipeline stages match "${searchQuery}".</p>`;
  }

  return filtered
    .map(
      (s) => `<div class="stage-row">
        <div class="stage-meta">
          <span class="stage-name">${s.name}</span>
          <span class="stage-time">${s.ms.toFixed(2)} ms · ${s.crateName}</span>
        </div>
        <div class="bar-track">
          <div class="bar-fill ${s.cached ? "bar-fill-green" : ""}" style="width:${s.pct}%;"></div>
        </div>
      </div>`,
    )
    .join("");
}

function mountDashboard(root: HTMLElement): void {
  const kpis = state.metrics
    .map(
      (m) => `<div class="kpi-cell">
        <div class="kpi-val">${m.value} <span>${m.unit}</span></div>
        <div class="kpi-lbl">${m.label}</div>
        <div class="kpi-delta">${m.delta}</div>
      </div>`,
    )
    .join("");

  const checks = state.checks
    .map(
      (c) => `<li>
        <span class="check-left">
          <span class="material-symbols-outlined notranslate" translate="no" style="color:var(--green);font-size:17px;">check_circle</span>
          <span>${c.label}</span>
        </span>
        <span class="check-ok">${c.badge}</span>
      </li>`,
    )
    .join("");

  root.innerHTML = `<div class="app-shell">
    <aside class="sidebar">
      <div>
        <div class="brand notranslate" translate="no">
          <span class="brand-icon"><span class="material-symbols-outlined">bolt</span></span>
          <span>zerho</span>
          <span class="brand-pill">v1.0.0</span>
        </div>
        <nav class="nav-group" aria-label="Dashboard navigation">
          <div class="nav-label">Telemetry</div>
          <button type="button" class="nav-btn" data-nav="overview" data-active="${activeTab === "overview"}">
            <span class="material-symbols-outlined notranslate" translate="no">dashboard</span>
            <span>Overview</span>
          </button>
          <button type="button" class="nav-btn" data-nav="pipeline" data-active="${activeTab === "pipeline"}">
            <span class="material-symbols-outlined notranslate" translate="no">speed</span>
            <span>Build Pipeline</span>
          </button>
          <button type="button" class="nav-btn" data-nav="security" data-active="${activeTab === "security"}">
            <span class="material-symbols-outlined notranslate" translate="no">verified_user</span>
            <span>Security &amp; SRI</span>
          </button>
        </nav>
      </div>
      <div class="sidebar-status">
        <div><span class="status-dot"></span><strong style="color:#fff;">Zero-Node Runtime</strong></div>
        <div style="margin-top:4px;font-family:var(--font-mono);font-size:0.72rem;">oxc · lightningcss · redb</div>
      </div>
    </aside>

    <div class="workspace">
      <header class="topbar">
        <div class="topbar-title">
          <h1>Zero-Node Telemetry Dashboard</h1>
          <div class="topbar-sub">examples/react-dashboard · live HMR + code-splitting demo</div>
        </div>
        <div class="topbar-controls">
          <label class="search-box">
            <span class="material-symbols-outlined notranslate" translate="no">search</span>
            <input id="stage-filter" type="search" placeholder="Filter stages or crates..." value="${searchQuery}" />
          </label>
          <button id="load-analytics" class="btn-primary" type="button" data-loaded="${analyticsLoaded}">
            <span class="material-symbols-outlined notranslate" translate="no">${analyticsLoaded ? "check" : "analytics"}</span>
            <span>${analyticsLoaded ? "Async Chunk Loaded" : "Load Async Chunk"}</span>
          </button>
        </div>
      </header>

      <main class="content">
        <section class="kpi-bar" aria-label="Key telemetry metrics">${kpis}</section>

        <section class="panel-grid">
          <div class="panel">
            <div class="panel-head">
              <h2>
                <span class="material-symbols-outlined notranslate" translate="no">bolt</span>
                <span>Native Rust Pipeline Latency</span>
              </h2>
              <span class="panel-tag">green = redb cached</span>
            </div>
            <div id="stages-list">${renderStages(state.stages)}</div>
          </div>

          <div class="panel">
            <div class="panel-head">
              <h2>
                <span class="material-symbols-outlined notranslate" translate="no">shield</span>
                <span>Production Hardening</span>
              </h2>
              <span class="panel-tag">100% Safe Rust</span>
            </div>
            <ul class="check-list">${checks}</ul>
          </div>
        </section>

        <div id="analytics-slot"></div>
      </main>
    </div>
  </div>`;

  const input = document.getElementById("stage-filter") as HTMLInputElement | null;
  const stagesEl = document.getElementById("stages-list");
  input?.addEventListener("input", () => {
    searchQuery = input.value;
    if (stagesEl) {
      stagesEl.innerHTML = renderStages(state.stages);
    }
  });

  document.querySelectorAll<HTMLButtonElement>("[data-nav]").forEach((btn) => {
    btn.addEventListener("click", () => {
      activeTab = btn.dataset.nav ?? "overview";
      document.querySelectorAll<HTMLButtonElement>("[data-nav]").forEach((b) => {
        b.dataset.active = String(b.dataset.nav === activeTab);
      });
    });
  });

  const loadBtn = document.getElementById("load-analytics");
  const slot = document.getElementById("analytics-slot");
  loadBtn?.addEventListener("click", async () => {
    const mod = await import("./AnalyticsPanel");
    analyticsLoaded = true;
    loadBtn.setAttribute("data-loaded", "true");
    loadBtn.innerHTML =
      '<span class="material-symbols-outlined notranslate" translate="no">check</span><span>Async Chunk Loaded</span>';
    if (slot) {
      slot.innerHTML = mod.default();
    }
  });
}

const rootEl = document.getElementById("root");
if (rootEl) {
  mountDashboard(rootEl);
}
