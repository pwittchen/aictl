import { render } from "solid-js/web";
import App from "./App";
import { ipc } from "./lib/ipc";
import { applyAppearance } from "./components/Settings";

const root = document.getElementById("root");
if (!root) {
  throw new Error("missing #root mount node");
}

// Hydrate theme + density before first paint so the dark/light flash is
// limited to the time the IPC call takes — much shorter than the agent
// loop, but still worth hiding behind a quick fetch on boot.
void Promise.all([
  ipc.configValue("AICTL_DESKTOP_THEME"),
  ipc.configValue("AICTL_DESKTOP_DENSITY"),
])
  .then(([theme, density]) =>
    applyAppearance({ theme: theme ?? "", density: density ?? "" }),
  )
  .catch(() => {
    // Boot failures fall back to the CSS defaults — better than blocking
    // render on a config read.
  });

render(() => <App />, root);
