import { render } from "solid-js/web";
import { App } from "./App.tsx";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("#root element is missing from index.html");
}
render(() => <App />, root);
