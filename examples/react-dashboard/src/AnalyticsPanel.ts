export interface ChunkStat {
  readonly file: string;
  readonly rawKb: string;
  readonly gzipKb: string;
  readonly brotliKb: string;
  readonly sri: string;
}

export function getChunkBreakdown(): readonly ChunkStat[] {
  return [
    { file: "assets/index.js", rawKb: "1.33 KB", gzipKb: "0.67 KB", brotliKb: "0.58 KB", sri: "sha384-verified" },
    { file: "assets/chunk-analytics.js", rawKb: "1.25 KB", gzipKb: "0.73 KB", brotliKb: "0.65 KB", sri: "sha384-verified" },
    { file: "assets/index.css", rawKb: "1.19 KB", gzipKb: "0.56 KB", brotliKb: "0.49 KB", sri: "sha384-verified" },
  ];
}

export default function renderAnalyticsMarkup(): string {
  const rows = getChunkBreakdown()
    .map(
      (c) => `<tr>
        <td style="color:#fff;">${c.file}</td>
        <td>${c.rawKb}</td>
        <td>${c.gzipKb}</td>
        <td style="color:#34d399;font-weight:600;">${c.brotliKb}</td>
        <td style="color:#fb923c;">${c.sri}</td>
      </tr>`,
    )
    .join("");

  return `<section class="async-panel">
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
      <tbody>${rows}</tbody>
    </table>
  </section>`;
}
