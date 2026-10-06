//! The Discord token lives in the OS credential store, next to Talk's Matrix
//! session, and never in Matrix.

use anyhow::{Result, anyhow, bail};
use keyring::Entry;

const SERVICE: &str = "chat.talk.Talk";
/// The linked Discord account. Until QR linking exists, the developer probe's
/// store-token command sets it.
const ACCOUNT: &str = "discord-token";
/// Where the first probe kept the token; moved to `ACCOUNT` when read.
const EARLIER_ACCOUNT: &str = "discord-probe-token";

fn entry() -> Result<Entry> {
    named(ACCOUNT)
}

fn named(account: &str) -> Result<Entry> {
    Entry::new(SERVICE, account).map_err(|_| anyhow!("could not access the OS credential store"))
}

pub fn store(token: &str) -> Result<()> {
    let token = clean(token)?;
    entry()?
        .set_password(&token)
        .map_err(|_| anyhow!("could not save the token in the OS credential store"))
}

pub fn load() -> Result<String> {
    if crate::client::test_server().is_some() {
        return Ok(crate::client::TEST_TOKEN.to_owned());
    }
    match entry()?.get_password() {
        Ok(token) => return Ok(token),
        Err(keyring::Error::NoEntry) => {}
        Err(_) => bail!("could not read the token from the OS credential store"),
    }
    match named(EARLIER_ACCOUNT)?.get_password() {
        Ok(token) => {
            store(&token)?;
            let _ = named(EARLIER_ACCOUNT)?.delete_credential();
            Ok(token)
        }
        Err(keyring::Error::NoEntry) => bail!("no Discord token saved; run store-token first"),
        Err(_) => bail!("could not read the token from the OS credential store"),
    }
}

pub fn forget() -> Result<()> {
    for account in [ACCOUNT, EARLIER_ACCOUNT] {
        match named(account)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(_) => bail!("could not remove the token from the OS credential store"),
        }
    }
    Ok(())
}

/// Accept the header value as copied from browser developer tools, with or
/// without surrounding quotes or whitespace.
fn clean(token: &str) -> Result<String> {
    let token = token.trim().trim_matches(['"', '\'']).trim();
    if token.is_empty() || token.contains(char::is_whitespace) {
        bail!("that doesn't look like a Discord token");
    }
    Ok(token.to_owned())
}

#[cfg(test)]
mod tests {
    use super::clean;

    #[test]
    fn cleans_copied_header_values() {
        assert_eq!(clean("  \"abc.def.ghi\"\n").unwrap(), "abc.def.ghi");
        assert!(clean("   ").is_err());
        assert!(clean("Bearer abc").is_err());
    }
}
