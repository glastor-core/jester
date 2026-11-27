const e={"src/AnalyticsPanel.ts":function(e,t){function n(){return[{file:`assets/index.js`,rawKb:`1.33 KB`,gzipKb:`0.67 KB`,brotliKb:`0.58 KB`,sri:`sha384-verified`},{file:`assets/chunk-analytics.js`,rawKb:`1.25 KB`,gzipKb:`0.73 KB`,brotliKb:`0.65 KB`,sri:`sha384-verified`},{file:`assets/index.css`,rawKb:`1.19 KB`,gzipKb:`0.56 KB`,brotliKb:`0.49 KB`,sri:`sha384-verified`}]}t.default=function(){return`<section class="async-panel">
    <div class="panel-head">
      <h2>
        <span class="material-symbols-outlined notranslate" translate="no">call_split</span>
        <span>Lazy-Loaded Async Chunk Telemetry</span>
      </h2>
      <span class="check-ok">dynamic import()</span>
    </div>
    <table class="chunk-table">
      <thead>
        <tr>
          <th>Emitted Asset</th>
          <th>Raw</th>
          <th>Gzip</th>
          <th>Brotli (.br)</th>
          <th>Integrity</th>
        </tr>
      </thead>
      <tbody>${n().map(e=>`<tr>
        <td style="color:#fff;">${e.file}</td>
        <td>${e.rawKb}</td>
        <td>${e.gzipKb}</td>
        <td style="color:#34d399;font-weight:600;">${e.brotliKb}</td>
        <td style="color:#fb923c;">${e.sri}</td>
      </tr>`).join(``)}</tbody>
    </table>
  </section>`},t.getChunkBreakdown=n}},t={};function n(r){if(t[r])return t[r].exports;let i=e[r];if(!i)throw Error(`[zerho] missing chunk module: `+r);let a={exports:{}};return t[r]=a,i(n,a.exports),a.exports.default&&(typeof a.exports.default==`object`||typeof a.exports.default==`function`)&&Object.keys(a.exports).length===1&&Object.assign(a.exports,a.exports.default),a.exports}const r=n(`src/AnalyticsPanel.ts`);export default r.default??r;
//# sourceMappingURL=chunk-080085f2.js.map
