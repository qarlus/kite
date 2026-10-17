// Typed wrappers over Tauri IPC. The only module in src/ that talks to the backend.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  Account, Action, AiChunk, Addr, Changed, ChatMsg, Draft, Event, Id, Message,
  ProtonLogin, Settings, Split, ThreadSummary, View,
} from "./types";

export const api = {
  accounts: () => invoke<Account[]>("accounts_list"),
  addProton: (login: ProtonLogin) => invoke<Account>("account_add_proton", { login }),
  addMicrosoft: () => invoke<Account>("account_add_microsoft"),
  removeAccount: (id: Id) => invoke<void>("account_remove", { id }),
  threads: (view: View, offset: number, limit: number) => invoke<ThreadSummary[]>("threads_list", { view, offset, limit }),
  thread: (id: Id) => invoke<Message[]>("thread_get", { id }),
  /** Applies locally at once and queues the remote op. Returns an undo token. */
  act: (ids: Id[], action: Action) => invoke<number>("thread_action", { ids, action }),
  undo: (token: number) => invoke<void>("undo", { token }),
  /** Queues to the outbox. Returns the outbox id, which is cancellable until sent. */
  send: (draft: Draft) => invoke<Id>("send", { draft }),
  cancelSend: (id: Id) => invoke<Draft>("outbox_cancel", { id }),
  contacts: (query: string) => invoke<Addr[]>("contacts", { query }),
  openAttachment: (id: Id) => invoke<void>("attachment_open", { id }),
  splits: () => invoke<Split[]>("splits_get"),
  setSplits: (splits: Split[]) => invoke<void>("splits_set", { splits }),
  calendar: (account: Id) => invoke<Event[]>("calendar_today", { account }),
  settings: () => invoke<Settings>("settings_get"),
  setSettings: (settings: Settings) => invoke<void>("settings_set", { settings }),
  setAiKey: (key: string) => invoke<void>("ai_key_set", { key }),
  /** Streams chunks over `kite://ai` tagged with `req`. */
  ai: (req: number, messages: ChatMsg[]) => invoke<void>("ai_chat", { req, messages }),
  sync: () => invoke<void>("sync_now"),
};

export const onChanged = (f: (c: Changed) => void) => listen<Changed>("kite://changed", (e) => f(e.payload));
export const onAi = (f: (c: AiChunk) => void) => listen<AiChunk>("kite://ai", (e) => f(e.payload));
