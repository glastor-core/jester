export interface PipelineRow {
  readonly stage: string;
  readonly ms: number;
  readonly status: string;
}

export function getPipelineBreakdown(): readonly PipelineRow[] {
  return [
    { stage: "oxc_resolver + tsconfig paths", ms: 0.42, status: "cached" },
    { stage: "oxc TS/JSX + Fast Refresh", ms: 0.55, status: "cached" },
    { stage: "lightningcss + @import/url()", ms: 0.31, status: "minified" },
    { stage: "Scope Hoisting + Brotli .br + SRI", ms: 4.18, status: "verified" },
  ];
}

export default function renderAnalyticsMarkup(): string {
  const rows = getPipelineBreakdown()
    .map(
      (r) =>
        `<tr><td>${r.stage}</td><td>${r.ms.toFixed(2)} ms</td><td>${r.status}</td></tr>`,
    )
    .join("");
  return `<section style="margin-top:20px;background:#111827;border:1px solid #1f2937;border-radius:12px;padding:18px;">
    <h2 style="margin:0 0 12px;font-family:'Red Hat Display',sans-serif;font-size:1.1rem;">Lazy-Loaded Pipeline Telemetry (Async Chunk)</h2>
    <table style="width:100%;border-collapse:collapse;font-family:'Red Hat Mono',monospace;font-size:0.84rem;">
      <thead><tr><th align="left">Stage</th><th align="left">Time</th><th align="left">Status</th></tr></thead>
      <tbody>${rows}</tbody>
    </table>
  </section>`;
}
