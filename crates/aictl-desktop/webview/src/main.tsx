import { render } from "solid-js/web";
import App from "./App";

const root = document.getElementById("root");
if (!root) {
  throw new Error("missing #root mount node");
}
render(() => <App />, root);
