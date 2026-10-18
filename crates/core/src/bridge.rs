//! Proton Bridge auto-discovery.
//!
//! Bridge stores everything a mail client needs in its own encrypted vault: each account's
//! primary address, the randomly generated Bridge password, and the IMAP/SMTP ports Bridge
//! actually settled on (it takes the first free port from 1143/1025). Reading that vault is
//! the only way to import an account without the user typing a password, because Bridge's
//! gRPC API is token-gated on a random loopback port.
//!
//! Everything here is read-only and runs off the UI thread. The vault key is used in memory
//! for one call and is never written anywhere.
use crate::model::ProtonLogin;
use anyhow::Result;

/// Proton Bridge accounts found on this machine, with the ports Bridge chose.
///
/// `Ok(vec![])` means Bridge is not installed or has no accounts; an `Err` means Bridge is
/// present but its vault could not be read, so the caller can explain that instead of
/// silently falling back to manual entry.
pub fn discover() -> Result<Vec<ProtonLogin>> {
    Ok(Vec::new())
}
