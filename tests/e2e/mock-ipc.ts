// Fake Tauri backend injected into the page before the app loads. Never imported by src/.
// Usage: `import { test, expect } from "./mock-ipc"`, then `await page.goto("/")`.
// Inspect calls with `await calls(page)`; push backend events with `await emit(page, "kite://changed", {...})`.
import { test as base, expect, type Page } from "@playwright/test";

export interface MockOpts { threads?: number; accounts?: number; overrides?: Record<string, unknown> }

function install(opts: MockOpts) {
  const now = Date.UTC(2026, 9, 17, 12, 0);
  const names = ["Alice Chen", "GitHub", "Bob Ortiz", "Linear", "Dana Kim", "Stripe", "Eve Park", "Figma"];
  const n = opts.threads ?? 50;
  const db = {
    accounts: Array.from({ length: opts.accounts ?? 1 }, (_, i) => ({
      id: i + 1, provider: i % 2 ? "microsoft" : "proton", email: `me${i}@kite.test`, name: `Me ${i}`,
    })),
    threads: Array.from({ length: n }, (_, i) => ({
      id: i + 1, account_id: 1, subject: `Subject ${i + 1}`, snippet: `Snippet for thread ${i + 1}`,
      participants: names[i % names.length], date: now - i * 3_600_000, unread: i % 3 === 0,
      starred: i % 7 === 0, important: i % 2 === 0, attachments: i % 5 === 0, labels: [], count: 1 + (i % 4),
      snoozed_until: null, mailbox: "inbox",
    })),
    splits: [{ id: 1, name: "Important", query: "is:important" }, { id: 2, name: "Other", query: "-is:important" }],
    settings: { theme: "dark", undo_send_secs: 5, load_remote_images: false, ai_enabled: false,
      ai_base_url: "https://opencode.ai/zen/go/v1", ai_model: "deepseek-v4.1-flash" },
    outbox: 0, undo: 0,
  };
  const calls: { cmd: string; args: any }[] = [];
  const cbs = new Map<number, (x: unknown) => void>();
  const listeners = new Map<string, number[]>();
  let cbId = 0;
  const visible = (view: any) => db.threads.filter((t: any) => {
    if (view.mailbox !== "all" && t.mailbox !== view.mailbox) return false;
    if (view.split === 1 && !t.important) return false;
    if (view.split === 2 && t.important) return false;
    if (view.query) return (t.subject + t.snippet + t.participants).toLowerCase().includes(view.query.toLowerCase());
    return true;
  });
  const handlers: Record<string, (a: any) => unknown> = {
    accounts_list: () => db.accounts,
    threads_list: ({ view, offset, limit }) => visible(view).slice(offset, offset + limit),
    thread_get: ({ id }) => {
      const t: any = db.threads.find((t: any) => t.id === id);
      return Array.from({ length: t.count }, (_, i) => ({
        id: id * 100 + i, thread_id: id, from: { name: t.participants, email: "x@kite.test" },
        to: [{ name: "Me", email: "me0@kite.test" }], cc: [], date: t.date - (t.count - i) * 60_000,
        subject: t.subject, html: i === t.count - 1 ? `<p>Hello <b>html</b> ${id}</p>` : null,
        text: `Body ${i} of ${id}`, attachments: [], unread: t.unread,
      }));
    },
    thread_action: ({ ids, action }) => {
      for (const t of db.threads as any[]) if (ids.includes(t.id)) {
        if (action.type === "archive") t.mailbox = "archive";
        if (action.type === "inbox") t.mailbox = "inbox";
        if (action.type === "trash") t.mailbox = "trash";
        if (action.type === "snooze") { t.mailbox = "snoozed"; t.snoozed_until = action.until; }
        if (action.type === "star" || action.type === "unstar") t.starred = action.type === "star";
        if (action.type === "read" || action.type === "unread") t.unread = action.type === "unread";
      }
      return ++db.undo;
    },
    undo: () => null,
    send: () => ++db.outbox,
    outbox_cancel: () => null,
    contacts: ({ query }) => names.filter((n) => n.toLowerCase().includes(query.toLowerCase()))
      .map((name) => ({ name, email: name.split(" ")[0].toLowerCase() + "@kite.test" })),
    splits_get: () => db.splits,
    splits_set: ({ splits }) => { db.splits = splits; },
    calendar_today: () => [],
    settings_get: () => db.settings,
    settings_set: ({ settings }) => { db.settings = settings; },
    sync_now: () => null,
    "plugin:event|listen": ({ event, handler }) => { listeners.set(event, [...(listeners.get(event) ?? []), handler]); return handler; },
    "plugin:event|unlisten": () => null,
    ...(opts.overrides ?? {}) as any,
  };
  const w = window as any;
  w.__KITE_CALLS__ = calls;
  w.__KITE_EMIT__ = (event: string, payload: unknown) => (listeners.get(event) ?? []).forEach((id) => cbs.get(id)?.({ event, id, payload }));
  w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
  w.__TAURI_INTERNALS__ = {
    metadata: { currentWindow: { label: "main" }, currentWebview: { windowLabel: "main", label: "main" } },
    transformCallback: (cb: (x: unknown) => void) => { cbs.set(++cbId, cb); return cbId; },
    unregisterCallback: (id: number) => cbs.delete(id),
    invoke: async (cmd: string, args: any = {}) => {
      calls.push({ cmd, args: structuredClone(args) });
      const h = handlers[cmd];
      if (!h) throw new Error(`mock-ipc: unhandled command ${cmd}`);
      return h(args);
    },
  };
}

export const test = base.extend<{ mock: MockOpts }>({
  mock: [{}, { option: true }],
  page: async ({ page, mock }, use) => {
    await page.addInitScript(install, mock);
    await use(page);
  },
});

export const calls = (page: Page) => page.evaluate(() => (window as any).__KITE_CALLS__ as { cmd: string; args: any }[]);
export const emit = (page: Page, event: string, payload: unknown) =>
  page.evaluate(([e, p]) => (window as any).__KITE_EMIT__(e, p), [event, payload] as const);
export { expect };
