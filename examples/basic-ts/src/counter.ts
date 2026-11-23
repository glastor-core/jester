export interface CounterState {
  count: number;
  label: string;
}

export function increment(state: CounterState): CounterState {
  return { ...state, count: state.count + 1 };
}

// Unused exports below should be eliminated by zerho's AST tree-shaker in production builds.
export function unusedHelper(value: number): number {
  return value * 42;
}

export const DEAD_BANNER = "NEVER_IN_PRODUCTION_BUNDLE";
