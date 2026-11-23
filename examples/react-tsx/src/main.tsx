import "./styles.css";
import { mount } from "react";
import badgePkg from "cjs-badge";

interface ZerhoHot {
  data: { chunkLoaded?: boolean };
  accept: () => void;
  dispose: (cb: (data: { chunkLoaded?: boolean }) => void) => void;
}

const hot = (import.meta as unknown as { hot?: ZerhoHot }).hot;
let chunkLoaded = hot?.data.chunkLoaded ?? false;

async function renderLazyChunk() {
  const mod = await import("./lazy-panel");
  const slot = document.getElementById("lazy-slot");
  if (slot) {
    const Panel = mod.default;
    mount(<Panel />, slot);
    chunkLoaded = true;
  }
}

function App() {
  return (
    <main className="shell">
      <h1>
        <span className="material-symbols-outlined">bolt</span>
        zerho React + TSX
      </h1>
      <p className="mono">{badgePkg.formatBadge("Zero-Node Rust Bundler")}</p>
      <button onClick={renderLazyChunk}>Load Split Async Chunk</button>
      <div id="lazy-slot" />
    </main>
  );
}

const root = document.getElementById("root");
if (root) {
  mount(<App />, root);
  if (chunkLoaded) {
    void renderLazyChunk();
  }
}

if (hot) {
  hot.dispose((data) => {
    data.chunkLoaded = chunkLoaded;
  });
  hot.accept();
}
