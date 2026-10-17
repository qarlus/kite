//! Tauri command handlers: thin glue from IPC to store/sync/outbox/ai. No logic lives here.
//! Stubs return an error until their owning module is wired in.
use crate::model::*;

type R<T> = Result<T, String>;

fn todo<T>() -> R<T> {
    Err("not implemented".into())
}

#[tauri::command] fn accounts_list() -> R<Vec<Account>> { Ok(vec![]) }
#[tauri::command] fn account_add_proton(login: ProtonLogin) -> R<Account> { let _ = login; todo() }
#[tauri::command] fn account_add_microsoft() -> R<Account> { todo() }
#[tauri::command] fn account_remove(id: Id) -> R<()> { let _ = id; todo() }
#[tauri::command] fn threads_list(view: View, offset: u32, limit: u32) -> R<Vec<ThreadSummary>> { let _ = (view, offset, limit); Ok(vec![]) }
#[tauri::command] fn thread_get(id: Id) -> R<Vec<Message>> { let _ = id; todo() }
#[tauri::command] fn thread_action(ids: Vec<Id>, action: Action) -> R<i64> { let _ = (ids, action); todo() }
#[tauri::command] fn undo(token: i64) -> R<()> { let _ = token; todo() }
#[tauri::command] fn send(draft: Draft) -> R<Id> { let _ = draft; todo() }
#[tauri::command] fn outbox_cancel(id: Id) -> R<Draft> { let _ = id; todo() }
#[tauri::command] fn contacts(query: String) -> R<Vec<Addr>> { let _ = query; Ok(vec![]) }
#[tauri::command] fn attachment_open(id: Id) -> R<()> { let _ = id; todo() }
#[tauri::command] fn splits_get() -> R<Vec<Split>> { Ok(vec![]) }
#[tauri::command] fn splits_set(splits: Vec<Split>) -> R<()> { let _ = splits; todo() }
#[tauri::command] fn calendar_today(account: Id) -> R<Vec<Event>> { let _ = account; Ok(vec![]) }
#[tauri::command] fn settings_get() -> R<Settings> { Ok(Settings::default()) }
#[tauri::command] fn settings_set(settings: Settings) -> R<()> { let _ = settings; todo() }
#[tauri::command] fn ai_key_set(key: String) -> R<()> { let _ = key; todo() }
#[tauri::command] fn ai_chat(req: u32, messages: Vec<ChatMsg>) -> R<()> { let _ = (req, messages); todo() }
#[tauri::command] fn sync_now() -> R<()> { Ok(()) }

pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        accounts_list, account_add_proton, account_add_microsoft, account_remove,
        threads_list, thread_get, thread_action, undo, send, outbox_cancel, contacts,
        attachment_open, splits_get, splits_set, calendar_today, settings_get, settings_set,
        ai_key_set, ai_chat, sync_now
    ]
}
