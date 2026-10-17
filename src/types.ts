// IPC contract. Mirrors src-tauri/src/model.rs exactly. Change both together or neither.
export type Id = number;
export type Ms = number;
export type Provider = "proton" | "microsoft";
export type Mailbox = "inbox" | "starred" | "snoozed" | "drafts" | "sent" | "archive" | "trash" | "spam" | "all";

export interface Account { id: Id; provider: Provider; email: string; name: string }
export interface View { account_id: Id | null; mailbox: Mailbox; split: Id | null; query: string | null }
export interface Addr { name: string; email: string }

export interface ThreadSummary {
  id: Id; account_id: Id; subject: string; snippet: string; participants: string; date: Ms;
  unread: boolean; starred: boolean; important: boolean; attachments: boolean;
  labels: string[]; count: number; snoozed_until: Ms | null;
}

export interface Attachment { id: Id; name: string; mime: string; size: number }
export interface Message {
  id: Id; thread_id: Id; from: Addr; to: Addr[]; cc: Addr[]; date: Ms; subject: string;
  html: string | null; text: string; attachments: Attachment[]; unread: boolean;
}

export type Action =
  | { type: "archive" } | { type: "inbox" } | { type: "trash" }
  | { type: "star" } | { type: "unstar" } | { type: "read" } | { type: "unread" }
  | { type: "snooze"; until: Ms } | { type: "move"; folder: string };

export interface Draft {
  account_id: Id; to: Addr[]; cc: Addr[]; bcc: Addr[]; subject: string; body: string;
  reply_to: Id | null; send_at: Ms | null; remind_at: Ms | null;
}

export interface Split { id: Id; name: string; query: string }
export interface Event { id: string; title: string; start: Ms; end: Ms; location: string | null; all_day: boolean }
export interface Settings {
  theme: "system" | "dark" | "light"; undo_send_secs: number; load_remote_images: boolean;
  ai_enabled: boolean; ai_base_url: string; ai_model: string;
}
export interface ProtonLogin { email: string; password: string; host: string | null; imap_port: number | null; smtp_port: number | null }
export interface ChatMsg { role: "system" | "user" | "assistant"; content: string }
export interface Changed { account_ids: Id[] }
export interface AiChunk { req: number; delta: string; done: boolean }
