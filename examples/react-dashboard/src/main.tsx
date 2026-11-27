import "./dashboard.css";
import { createInitialState, type PipelineStage, type SecurityCheck } from "./store";

type TabId = "overview" | "pipeline" | "security";
type CacheFilter = "all" | "cached" | "cold";

const state = createInitialState();
let activeTab: TabId = "overview";
let searchQuery = "";
let cacheFilter: CacheFilter = "all";
let analyticsLoaded = false;
let analyticsMarkup = "";

const TAB_META: Record<TabId, { title: string; subtitle: string; placeholder: string }> = {
  overview: {
    title: "Zero-Node Telemetry Dashboard",
    subtitle: "examples/react-dashboard · live HMR + code-splitting demo",
    placeholder: "Filter stages or crates...",
  },
  pipeline: {
    title: "Native Rust Build Pipeline",
    subtitle: "oxc + lightningcss + redb · per-stage waterfall & crate ledger",
    placeholder: "Filter stages or crates...",
  },
  security: {
    title: "Production Security & SRI",
    subtitle: "SHA-384 integrity · dist/csp.json · zero-leak guard · Wasmtime sandbox",
    placeholder: "Filter security checks...",
  },
};

function filterStages(stages: readonly PipelineStage[]): readonly PipelineStage[] {
  const q = searchQuery.trim().toLowerCase();
  return stages.filter((s) => {
    if (activeTab === "pipeline") {
      if (cacheFilter === "cached" && !s.cached) return false;
      if (cacheFilter === "cold" && s.cached) return false;
    }
    if (!q) return true;
    return s.name.toLowerCase().includes(q) || s.crateName.toLowerCase().includes(q);
  });
}

function renderStages(stages: readonly PipelineStage[]): string {
  const filtered = filterStages(stages);
  if (filtered.length === 0) {
    return `<p style="font-size:0.84rem;color:var(--text-dim);margin:8px 0;">No pipeline stages match the current filter.</p>`;
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

function renderChecks(checks: readonly SecurityCheck[], detailed: boolean): string {
  const q = activeTab === "security" ? searchQuery.trim().toLowerCase() : "";
  const filtered = q
    ? checks.filter(
        (c) =>
          c.label.toLowerCase().includes(q) ||
          c.badge.toLowerCase().includes(q) ||
          c.detail.toLowerCase().includes(q),
      )
    : checks;

  if (filtered.length === 0) {
    return `<li style="color:var(--text-dim);">No security checks match "${searchQuery}".</li>`;
  }

  return filtered
    .map(
      (c) => `<li>
        <span class="check-left">
          <span class="material-symbols-outlined notranslate" translate="no" style="color:var(--green);font-size:17px;">check_circle</span>
          <span>
            ${c.label}
            ${detailed ? `<span class="check-detail">${c.detail}</span>` : ""}
          </span>
        </span>
        <span class="check-ok">${c.badge}</span>
      </li>`,
    )
    .join("");
}

function renderOverviewView(): string {
  const kpis = state.metrics
    .map(
      (m) => `<div class="kpi-cell">
        <div class="kpi-val">${m.value} <span>${m.unit}</span></div>
        <div class="kpi-lbl">${m.label}</div>
        <div class="kpi-delta">${m.delta}</div>
      </div>`,
    )
    .join("");

  return `
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
        <ul class="check-list">${renderChecks(state.checks, false)}</ul>
      </div>
    </section>

    <div id="analytics-slot">${analyticsMarkup}</div>
  `;
}

function renderPipelineView(): string {
  const visibleStages = filterStages(state.stages);
  const totalMs = visibleStages.reduce((acc, s) => acc + s.ms, 0).toFixed(2);
  const crateRows = state.crates
    .map(
      (c) => `<tr>
        <td style="color:#fff;font-weight:600;">${c.name}</td>
        <td style="font-family:var(--font-body);color:var(--text-muted);">${c.role}</td>
        <td style="color:var(--accent-bright);">${c.avgMs}</td>
        <td><span class="check-ok">${c.safety}</span></td>
      </tr>`,
    )
    .join("");

  const asyncSection = analyticsLoaded
    ? `<div id="analytics-slot">${analyticsMarkup}</div>`
    : `<div id="analytics-slot">
        <div class="empty-cta">
          <span>Dynamic code-splitting chunk <code style="font-family:var(--font-mono);color:#fff;">./AnalyticsPanel.ts</code> is not loaded yet.</span>
          <button type="button" class="btn-primary" data-trigger-analytics>
            <span class="material-symbols-outlined notranslate" translate="no">analytics</span>
            <span>Load Async Chunk Now</span>
          </button>
        </div>
      </div>`;

  return `<div class="view-stack">
    <section class="panel">
      <div class="panel-head">
        <h2>
          <span class="material-symbols-outlined notranslate" translate="no">speed</span>
          <span>Pipeline Stage Waterfall</span>
        </h2>
        <div class="filter-pills" role="group" aria-label="Cache filter">
          <button type="button" class="filter-pill" data-cache-filter="all" data-active="${cacheFilter === "all"}">All (${state.stages.length})</button>
          <button type="button" class="filter-pill" data-cache-filter="cached" data-active="${cacheFilter === "cached"}">redb Cached</button>
          <button type="button" class="filter-pill" data-cache-filter="cold" data-active="${cacheFilter === "cold"}">Cold Transform</button>
          <span class="panel-tag">total: ${totalMs} ms</span>
        </div>
      </div>
      <div id="stages-list">${renderStages(state.stages)}</div>
    </section>

    <section class="panel">
      <div class="panel-head">
        <h2>
          <span class="material-symbols-outlined notranslate" translate="no">inventory_2</span>
          <span>Active Rust Workspace Crates</span>
        </h2>
        <span class="panel-tag">unsafe_code = "forbid"</span>
      </div>
      <table class="chunk-table">
        <thead>
          <tr>
            <th>Crate</th>
            <th>Responsibility</th>
            <th>Avg Stage</th>
            <th>Memory Safety</th>
          </tr>
        </thead>
        <tbody>${crateRows}</tbody>
      </table>
    </section>

    ${asyncSection}
  </div>`;
}

function renderSecurityView(): string {
  return `<div class="view-stack">
    <section class="panel">
      <div class="panel-head">
        <h2>
          <span class="material-symbols-outlined notranslate" translate="no">verified_user</span>
          <span>Production Hardening Checklist</span>
        </h2>
        <span class="panel-tag">5 / 5 active</span>
      </div>
      <ul id="security-list" class="check-list">${renderChecks(state.checks, true)}</ul>
    </section>

    <section class="spec-grid">
      <div class="panel">
        <div class="panel-head">
          <h2>
            <span class="material-symbols-outlined notranslate" translate="no">fingerprint</span>
            <span>SHA-384 Subresource Integrity</span>
          </h2>
          <span class="panel-tag">index.html</span>
        </div>
        <pre class="code-pre notranslate" translate="no">&lt;link rel="stylesheet" href="/assets/index.css"
  <span class="code-accent">integrity="sha384-9f8e...verified"</span>
  crossorigin="anonymous"&gt;
&lt;script type="module" src="/assets/index.js"
  <span class="code-accent">integrity="sha384-4a2c...verified"</span>
  crossorigin="anonymous"&gt;&lt;/script&gt;</pre>
      </div>

      <div class="panel">
        <div class="panel-head">
          <h2>
            <span class="material-symbols-outlined notranslate" translate="no">policy</span>
            <span>Generated CSP Manifest</span>
          </h2>
          <span class="panel-tag">dist/csp.json</span>
        </div>
        <pre class="code-pre notranslate" translate="no">{
  <span class="code-green">"default-src"</span>: ["'self'"],
  <span class="code-green">"script-src"</span>:  ["'self'", "'sha384-4a2c...'"],
  <span class="code-green">"style-src"</span>:   ["'self'", "'sha384-9f8e...'"],
  <span class="code-green">"object-src"</span>:  ["'none'"],
  <span class="code-green">"base-uri"</span>:    ["'self'"]
}</pre>
      </div>
    </section>
  </div>`;
}

async function triggerAsyncChunk(): Promise<void> {
  const mod = await import("./AnalyticsPanel");
  analyticsLoaded = true;
  analyticsMarkup = mod.default();

  const loadBtn = document.getElementById("load-analytics");
  if (loadBtn) {
    loadBtn.setAttribute("data-loaded", "true");
    loadBtn.innerHTML =
      '<span class="material-symbols-outlined notranslate" translate="no">check</span><span>Async Chunk Loaded</span>';
  }

  const slot = document.getElementById("analytics-slot");
  if (slot) {
    slot.innerHTML = analyticsMarkup;
  }
}

function renderActiveView(): void {
  const viewRoot = document.getElementById("view-root");
  if (!viewRoot) return;

  const meta = TAB_META[activeTab];
  const titleEl = document.getElementById("topbar-heading");
  const subEl = document.getElementById("topbar-subtitle");
  const input = document.getElementById("stage-filter") as HTMLInputElement | null;
  if (titleEl) titleEl.textContent = meta.title;
  if (subEl) subEl.textContent = meta.subtitle;
  if (input) input.placeholder = meta.placeholder;

  if (activeTab === "overview") {
    viewRoot.innerHTML = renderOverviewView();
  } else if (activeTab === "pipeline") {
    viewRoot.innerHTML = renderPipelineView();
    viewRoot.querySelectorAll<HTMLButtonElement>("[data-cache-filter]").forEach((btn) => {
      btn.addEventListener("click", () => {
        cacheFilter = (btn.dataset.cacheFilter as CacheFilter) ?? "all";
        renderActiveView();
      });
    });
    const inlineTrigger = viewRoot.querySelector<HTMLButtonElement>("[data-trigger-analytics]");
    inlineTrigger?.addEventListener("click", () => {
      void triggerAsyncChunk();
    });
  } else {
    viewRoot.innerHTML = renderSecurityView();
  }
}

function mountDashboard(root: HTMLElement): void {
  root.innerHTML = `<div class="app-shell">
    <aside class="sidebar">
      <div>
        <div class="brand notranslate" translate="no">
          <span class="brand-icon"><span class="material-symbols-outlined">bolt</span></span>
          <span>zerho</span>
          <span class="brand-pill">v1.1.0</span>
        </div>
        <nav class="nav-group" aria-label="Dashboard navigation">
          <div class="nav-label">Telemetry</div>
          <button type="button" class="nav-btn" data-nav="overview" data-active="true">
            <span class="material-symbols-outlined notranslate" translate="no">dashboard</span>
            <span>Overview</span>
          </button>
          <button type="button" class="nav-btn" data-nav="pipeline" data-active="false">
            <span class="material-symbols-outlined notranslate" translate="no">speed</span>
            <span>Build Pipeline</span>
          </button>
          <button type="button" class="nav-btn" data-nav="security" data-active="false">
            <span class="material-symbols-outlined notranslate" translate="no">verified_user</span>
            <span>Security &amp; SRI</span>
          </button>
        </nav>
      </div>
      <div class="sidebar-footer">
        <a href="/" class="back-link">
          <span class="material-symbols-outlined notranslate" translate="no">arrow_back</span>
          <span>Back to zerho.xyz</span>
        </a>
        <div class="sidebar-status">
          <div><span class="status-dot"></span><strong style="color:#fff;">Zero-Node Runtime</strong></div>
          <div style="margin-top:4px;font-family:var(--font-mono);font-size:0.72rem;">oxc · lightningcss · redb</div>
        </div>
      </div>
    </aside>

    <div class="workspace">
      <header class="topbar">
        <div class="topbar-title">
          <h1 id="topbar-heading">${TAB_META.overview.title}</h1>
          <div id="topbar-subtitle" class="topbar-sub">${TAB_META.overview.subtitle}</div>
        </div>
        <div class="topbar-controls">
          <label class="search-box">
            <span class="material-symbols-outlined notranslate" translate="no">search</span>
            <input id="stage-filter" type="search" placeholder="${TAB_META.overview.placeholder}" value="${searchQuery}" />
          </label>
          <button id="load-analytics" class="btn-primary" type="button" data-loaded="${analyticsLoaded}">
            <span class="material-symbols-outlined notranslate" translate="no">${analyticsLoaded ? "check" : "analytics"}</span>
            <span>${analyticsLoaded ? "Async Chunk Loaded" : "Load Async Chunk"}</span>
          </button>
        </div>
      </header>

      <main id="view-root" class="content"></main>
    </div>
  </div>`;

  renderActiveView();

  const input = document.getElementById("stage-filter") as HTMLInputElement | null;
  input?.addEventListener("input", () => {
    searchQuery = input.value;
    if (activeTab === "security") {
      const secList = document.getElementById("security-list");
      if (secList) {
        secList.innerHTML = renderChecks(state.checks, true);
      }
      return;
    }
    const stagesEl = document.getElementById("stages-list");
    if (stagesEl) {
      stagesEl.innerHTML = renderStages(state.stages);
    }
  });

  document.querySelectorAll<HTMLButtonElement>("[data-nav]").forEach((btn) => {
    btn.addEventListener("click", () => {
      activeTab = (btn.dataset.nav as TabId) ?? "overview";
      document.querySelectorAll<HTMLButtonElement>("[data-nav]").forEach((b) => {
        b.dataset.active = String(b.dataset.nav === activeTab);
      });
      renderActiveView();
    });
  });

  const loadBtn = document.getElementById("load-analytics");
  loadBtn?.addEventListener("click", () => {
    void triggerAsyncChunk();
  });
}

const rootEl = document.getElementById("root");
if (rootEl) {
  mountDashboard(rootEl);
}
