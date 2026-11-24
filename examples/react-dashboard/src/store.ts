export interface MetricItem {
  readonly id: string;
  readonly label: string;
  readonly value: string;
}

export interface DashboardState {
  readonly filter: string;
  readonly metrics: readonly MetricItem[];
}

export function createInitialState(): DashboardState {
  return {
    filter: "all",
    metrics: [
      { id: "cache", label: "Warm redb Lookup", value: "1.7 us" },
      { id: "tsx", label: "Cold Oxc TSX", value: "0.548 ms" },
      { id: "build", label: "Warm Prod Build", value: "19.32 ms" },
      { id: "bin", label: "Native Rust Binary", value: "16.6 MB" },
    ],
  };
}
