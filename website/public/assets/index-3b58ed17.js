(function(){function e(){return{metrics:[{id:`cache`,value:`1.7`,unit:`us`,label:`Warm redb Lookup`,delta:`ACID B-tree hit`},{id:`tsx`,value:`0.548`,unit:`ms`,label:`Cold Oxc TSX`,delta:`0 Node processes`},{id:`build`,value:`19.32`,unit:`ms`,label:`Warm Prod Build`,delta:`Scope Hoisted`},{id:`bin`,value:`16.6`,unit:`MB`,label:`Native Binary`,delta:`unsafe_code = forbid`}],stages:[{name:`oxc_resolver + tsconfig paths`,crateName:`zerho-resolver`,ms:.42,pct:18,cached:!0},{name:`oxc TS/JSX + React Fast Refresh`,crateName:`zerho-transform`,ms:.55,pct:24,cached:!0},{name:`lightningcss + @import/url()`,crateName:`zerho-transform`,ms:.31,pct:14,cached:!1},{name:`Barrel Optimizer + CJS Pre-Bundler`,crateName:`zerho-transform`,ms:.64,pct:28,cached:!0},{name:`Scope Hoisting + Brotli .br + SRI`,crateName:`zerho-bundler`,ms:4.18,pct:86,cached:!1}],checks:[{label:`SHA-384 Subresource Integrity`,badge:`verified`,detail:`Injects integrity="sha384-..." on all emitted JS/CSS tags in index.html.`},{label:`Strict dist/csp.json Manifest`,badge:`active`,detail:`Generates hash-pinned Content-Security-Policy directives for edge headers.`},{label:`Client Secret Leak Guard`,badge:`0 leaks`,detail:`Scans bundled client chunks for cloud keys, tokens, and PEM headers.`},{label:`Brotli .br Pre-Compression`,badge:`-56% size`,detail:`Writes static .br sidecars ready for Cloudflare, Nginx, and Caddy.`},{label:`Wasmtime Fuel-Metered Sandbox`,badge:`1M fuel`,detail:`Executes WASM plugins with strict CPU fuel budgets and zero host FS access.`}],crates:[{name:`zerho-resolver`,role:`ESM/CJS resolution, package.json exports & tsconfig paths`,avgMs:`0.42 ms`,safety:`0 unsafe`},{name:`zerho-transform`,role:`Oxc TS/JSX, Fast Refresh, lightningcss & CSS @import`,avgMs:`1.50 ms`,safety:`0 unsafe`},{name:`zerho-bundler`,role:`Scope hoisting, code splitting, Brotli .br, SRI & CSP`,avgMs:`4.18 ms`,safety:`0 unsafe`},{name:`zerho-cache`,role:`Persistent ACID redb B-tree & remote HTTP CAS client`,avgMs:`0.002 ms`,safety:`0 unsafe`},{name:`zerho-plugin`,role:`Wasmtime UTF-8 linear-memory plugin host (Vue/Svelte/Solid)`,avgMs:`0.18 ms`,safety:`0 unsafe`}]}}let t=e(),n=`overview`,r=``,i=`all`,a=!1,o=``,s={overview:{title:`Zero-Node Telemetry Dashboard`,subtitle:`examples/react-dashboard · live HMR + code-splitting demo`,placeholder:`Filter stages or crates...`},pipeline:{title:`Native Rust Build Pipeline`,subtitle:`oxc + lightningcss + redb · per-stage waterfall & crate ledger`,placeholder:`Filter stages or crates...`},security:{title:`Production Security & SRI`,subtitle:`SHA-384 integrity · dist/csp.json · zero-leak guard · Wasmtime sandbox`,placeholder:`Filter security checks...`}};function c(e){let t=r.trim().toLowerCase();return e.filter(e=>n===`pipeline`&&(i===`cached`&&!e.cached||i===`cold`&&e.cached)?!1:!t||e.name.toLowerCase().includes(t)||e.crateName.toLowerCase().includes(t))}function l(e){let t=c(e);return t.length===0?`<p style="font-size:0.84rem;color:var(--text-dim);margin:8px 0;">No pipeline stages match the current filter.</p>`:t.map(e=>`<div class="stage-row">
        <div class="stage-meta">
          <span class="stage-name">${e.name}</span>
          <span class="stage-time">${e.ms.toFixed(2)} ms · ${e.crateName}</span>
        </div>
        <div class="bar-track">
          <div class="bar-fill ${e.cached?`bar-fill-green`:``}" style="width:${e.pct}%;"></div>
        </div>
      </div>`).join(``)}function u(e,t){let i=n===`security`?r.trim().toLowerCase():``,a=i?e.filter(e=>e.label.toLowerCase().includes(i)||e.badge.toLowerCase().includes(i)||e.detail.toLowerCase().includes(i)):e;return a.length===0?`<li style="color:var(--text-dim);">No security checks match "${r}".</li>`:a.map(e=>`<li>
        <span class="check-left">
          <span class="material-symbols-outlined notranslate" translate="no" style="color:var(--green);font-size:17px;">check_circle</span>
          <span>
            ${e.label}
            ${t?`<span class="check-detail">${e.detail}</span>`:``}
          </span>
        </span>
        <span class="check-ok">${e.badge}</span>
      </li>`).join(``)}function d(){return`
    <section class="kpi-bar" aria-label="Key telemetry metrics">${t.metrics.map(e=>`<div class="kpi-cell">
        <div class="kpi-val">${e.value} <span>${e.unit}</span></div>
        <div class="kpi-lbl">${e.label}</div>
        <div class="kpi-delta">${e.delta}</div>
      </div>`).join(``)}</section>

    <section class="panel-grid">
      <div class="panel">
        <div class="panel-head">
          <h2>
            <span class="material-symbols-outlined notranslate" translate="no">bolt</span>
            <span>Native Rust Pipeline Latency</span>
          </h2>
          <span class="panel-tag">green = redb cached</span>
        </div>
        <div id="stages-list">${l(t.stages)}</div>
      </div>

      <div class="panel">
        <div class="panel-head">
          <h2>
            <span class="material-symbols-outlined notranslate" translate="no">shield</span>
            <span>Production Hardening</span>
          </h2>
          <span class="panel-tag">100% Safe Rust</span>
        </div>
        <ul class="check-list">${u(t.checks,!1)}</ul>
      </div>
    </section>

    <div id="analytics-slot">${o}</div>
  `}function f(){let e=c(t.stages).reduce((e,t)=>e+t.ms,0).toFixed(2),n=t.crates.map(e=>`<tr>
        <td style="color:#fff;font-weight:600;">${e.name}</td>
        <td style="font-family:var(--font-body);color:var(--text-muted);">${e.role}</td>
        <td style="color:var(--accent-bright);">${e.avgMs}</td>
        <td><span class="check-ok">${e.safety}</span></td>
      </tr>`).join(``),r=a?`<div id="analytics-slot">${o}</div>`:`<div id="analytics-slot">
        <div class="empty-cta">
          <span>Dynamic code-splitting chunk <code style="font-family:var(--font-mono);color:#fff;">./AnalyticsPanel.ts</code> is not loaded yet.</span>
          <button type="button" class="btn-primary" data-trigger-analytics>
            <span class="material-symbols-outlined notranslate" translate="no">analytics</span>
            <span>Load Async Chunk Now</span>
          </button>
        </div>
      </div>`;return`<div class="view-stack">
    <section class="panel">
      <div class="panel-head">
        <h2>
          <span class="material-symbols-outlined notranslate" translate="no">speed</span>
          <span>Pipeline Stage Waterfall</span>
        </h2>
        <div class="filter-pills" role="group" aria-label="Cache filter">
          <button type="button" class="filter-pill" data-cache-filter="all" data-active="${i===`all`}">All (${t.stages.length})</button>
          <button type="button" class="filter-pill" data-cache-filter="cached" data-active="${i===`cached`}">redb Cached</button>
          <button type="button" class="filter-pill" data-cache-filter="cold" data-active="${i===`cold`}">Cold Transform</button>
          <span class="panel-tag">total: ${e} ms</span>
        </div>
      </div>
      <div id="stages-list">${l(t.stages)}</div>
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
        <tbody>${n}</tbody>
      </table>
    </section>

    ${r}
  </div>`}function p(){return`<div class="view-stack">
    <section class="panel">
      <div class="panel-head">
        <h2>
          <span class="material-symbols-outlined notranslate" translate="no">verified_user</span>
          <span>Production Hardening Checklist</span>
        </h2>
        <span class="panel-tag">5 / 5 active</span>
      </div>
      <ul id="security-list" class="check-list">${u(t.checks,!0)}</ul>
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
  </div>`}async function m(){let e=await import(`/assets/chunk-e9d67afc.js`);a=!0,o=e.default();let t=document.getElementById(`load-analytics`);t&&(t.setAttribute(`data-loaded`,`true`),t.innerHTML=`<span class="material-symbols-outlined notranslate" translate="no">check</span><span>Async Chunk Loaded</span>`);let n=document.getElementById(`analytics-slot`);n&&(n.innerHTML=o)}function h(){let e=document.getElementById(`view-root`);if(!e)return;let t=s[n],r=document.getElementById(`topbar-heading`),a=document.getElementById(`topbar-subtitle`),o=document.getElementById(`stage-filter`);r&&(r.textContent=t.title),a&&(a.textContent=t.subtitle),o&&(o.placeholder=t.placeholder),n===`overview`?e.innerHTML=d():n===`pipeline`?(e.innerHTML=f(),e.querySelectorAll(`[data-cache-filter]`).forEach(e=>{e.addEventListener(`click`,()=>{i=e.dataset.cacheFilter??`all`,h()})}),e.querySelector(`[data-trigger-analytics]`)?.addEventListener(`click`,()=>{m()})):e.innerHTML=p()}function g(e){e.innerHTML=`<div class="app-shell">
    <aside class="sidebar">
      <div>
        <div class="brand notranslate" translate="no">
          <span class="brand-icon"><span class="material-symbols-outlined">bolt</span></span>
          <span>zerho</span>
          <span class="brand-pill">v1.0.0</span>
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
          <h1 id="topbar-heading">${s.overview.title}</h1>
          <div id="topbar-subtitle" class="topbar-sub">${s.overview.subtitle}</div>
        </div>
        <div class="topbar-controls">
          <label class="search-box">
            <span class="material-symbols-outlined notranslate" translate="no">search</span>
            <input id="stage-filter" type="search" placeholder="${s.overview.placeholder}" value="${r}" />
          </label>
          <button id="load-analytics" class="btn-primary" type="button" data-loaded="${a}">
            <span class="material-symbols-outlined notranslate" translate="no">${a?`check`:`analytics`}</span>
            <span>${a?`Async Chunk Loaded`:`Load Async Chunk`}</span>
          </button>
        </div>
      </header>

      <main id="view-root" class="content"></main>
    </div>
  </div>`,h();let i=document.getElementById(`stage-filter`);i?.addEventListener(`input`,()=>{if(r=i.value,n===`security`){let e=document.getElementById(`security-list`);e&&(e.innerHTML=u(t.checks,!0));return}let e=document.getElementById(`stages-list`);e&&(e.innerHTML=l(t.stages))}),document.querySelectorAll(`[data-nav]`).forEach(e=>{e.addEventListener(`click`,()=>{n=e.dataset.nav??`overview`,document.querySelectorAll(`[data-nav]`).forEach(e=>{e.dataset.active=String(e.dataset.nav===n)}),h()})}),document.getElementById(`load-analytics`)?.addEventListener(`click`,()=>{m()})}let _=document.getElementById(`root`);_&&g(_)})();
//# sourceMappingURL=index-3b58ed17.js.map
