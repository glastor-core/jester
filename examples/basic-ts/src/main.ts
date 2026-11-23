import "./styles.css";
import { increment, type CounterState } from "./counter";

interface ZerhoHot {
  data: { count?: number };
  accept: () => void;
  dispose: (cb: (data: { count?: number }) => void) => void;
}

const hot = (import.meta as unknown as { hot?: ZerhoHot }).hot;

let state: CounterState = {
  count: hot?.data.count ?? 0,
  label: "Compiled natively by Oxc + LightningCSS (Zero Node.js)",
};

const env = (import.meta as unknown as { env?: Record<string, string> }).env;
const title = env?.ZERHO_APP_TITLE ?? "zerho dev server";

const root = document.getElementById("app");
if (root) {
  root.innerHTML = `
    <main class="card">
      <h1><img src="/logo.svg" alt="zerho" width="32" height="32" /> ${title}</h1>
      <p class="badge">${state.label}</p>
      <button id="counter-btn">Clicks: ${state.count}</button>
    </main>
  `;

  const button = document.getElementById("counter-btn");
  button?.addEventListener("click", () => {
    state = increment(state);
    button.textContent = `Clicks: ${state.count}`;
  });
}

if (hot) {
  hot.dispose((data) => {
    data.count = state.count;
  });
  hot.accept();
}
