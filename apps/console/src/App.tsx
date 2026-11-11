import { loadRenderer } from "@cavea/renderer";
import { createResource, Match, Switch } from "solid-js";

export function App() {
  const [renderer] = createResource(() => loadRenderer());
  return (
    <main>
      <h1>Cavea Console</h1>
      <Switch>
        <Match when={renderer.error !== undefined}>
          <p role="alert">Renderer failed to load: {String(renderer.error)}</p>
        </Match>
        <Match when={renderer()}>{(loaded) => <p>Renderer version {loaded().version()}</p>}</Match>
        <Match when={true}>
          <p>Loading renderer…</p>
        </Match>
      </Switch>
    </main>
  );
}
