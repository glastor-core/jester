export interface MetricItem {
  readonly id: string;
  readonly value: string;
  readonly unit: string;
  readonly label: string;
  readonly delta: string;
}

export interface PipelineStage {
  readonly name: string;
  readonly crateName: string;
  readonly ms: number;
  readonly pct: number;
  readonly cached: boolean;
}

export interface SecurityCheck {
  readonly label: string;
  readonly badge: string;
  readonly detail: string;
}

export interface CrateEntry {
  readonly name: string;
  readonly role: string;
  readonly avgMs: string;
  readonly safety: string;
}

export interface DashboardState {
  readonly metrics: readonly MetricItem[];
  readonly stages: readonly PipelineStage[];
  readonly checks: readonly SecurityCheck[];
  readonly crates: readonly CrateEntry[];
}

export function createInitialState(): DashboardState {
  return {
    metrics: [
      { id: "cache", value: "1.7", unit: "us", label: "Warm redb Lookup", delta: "ACID B-tree hit" },
      { id: "tsx", value: "0.548", unit: "ms", label: "Cold Oxc TSX", delta: "0 Node processes" },
      { id: "build", value: "19.32", unit: "ms", label: "Warm Prod Build", delta: "Scope Hoisted" },
      { id: "bin", value: "16.6", unit: "MB", label: "Native Binary", delta: "unsafe_code = forbid" },
    ],
    stages: [
      { name: "oxc_resolver + tsconfig paths", crateName: "zerho-resolver", ms: 0.42, pct: 18, cached: true },
      { name: "oxc TS/JSX + React Fast Refresh", crateName: "zerho-transform", ms: 0.55, pct: 24, cached: true },
      { name: "lightningcss + @import/url()", crateName: "zerho-transform", ms: 0.31, pct: 14, cached: false },
      { name: "Barrel Optimizer + CJS Pre-Bundler", crateName: "zerho-transform", ms: 0.64, pct: 28, cached: true },
      { name: "Scope Hoisting + Brotli .br + SRI", crateName: "zerho-bundler", ms: 4.18, pct: 86, cached: false },
    ],
    checks: [
      {
        label: "SHA-384 Subresource Integrity",
        badge: "verified",
        detail: "Injects integrity=\"sha384-...\" on all emitted JS/CSS tags in index.html.",
      },
      {
        label: "Strict dist/csp.json Manifest",
        badge: "active",
        detail: "Generates hash-pinned Content-Security-Policy directives for edge headers.",
      },
      {
        label: "Client Secret Leak Guard",
        badge: "0 leaks",
        detail: "Scans bundled client chunks for cloud keys, tokens, and PEM headers.",
      },
      {
        label: "Brotli .br Pre-Compression",
        badge: "-56% size",
        detail: "Writes static .br sidecars ready for Cloudflare, Nginx, and Caddy.",
      },
      {
        label: "Wasmtime Fuel-Metered Sandbox",
        badge: "1M fuel",
        detail: "Executes WASM plugins with strict CPU fuel budgets and zero host FS access.",
      },
    ],
    crates: [
      { name: "zerho-resolver", role: "ESM/CJS resolution, package.json exports & tsconfig paths", avgMs: "0.42 ms", safety: "0 unsafe" },
      { name: "zerho-transform", role: "Oxc TS/JSX, Fast Refresh, lightningcss & CSS @import", avgMs: "1.50 ms", safety: "0 unsafe" },
      { name: "zerho-bundler", role: "Scope hoisting, code splitting, Brotli .br, SRI & CSP", avgMs: "4.18 ms", safety: "0 unsafe" },
      { name: "zerho-cache", role: "Persistent ACID redb B-tree & remote HTTP CAS client", avgMs: "0.002 ms", safety: "0 unsafe" },
      { name: "zerho-plugin", role: "Wasmtime UTF-8 linear-memory plugin host (Vue/Svelte/Solid)", avgMs: "0.18 ms", safety: "0 unsafe" },
    ],
  };
}
