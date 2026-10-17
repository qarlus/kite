import { createResource, For } from "solid-js";
import { api } from "./ipc";

export default function App() {
  const [threads] = createResource(() => api.threads({ account_id: null, mailbox: "inbox", split: null, query: null }, 0, 200));
  return (
    <main role="listbox" aria-label="Threads">
      <For each={threads()}>{(t) => <div role="option">{t.subject}</div>}</For>
    </main>
  );
}
