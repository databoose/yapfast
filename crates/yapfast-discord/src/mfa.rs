//! Two-factor authentication for a request Discord holds back until it's
//! confirmed, as Discord's client does it: Discord answers 401 with code
//! 60003 and a ticket; the person confirms with one of the ticket's
//! methods, which `POST /mfa/finish` turns into a token; the request goes
//! again with the token in `X-Discord-MFA-Authorization`. Discord asks this
//! of moderation in a server that requires 2FA for it, and of an owner's
//! own actions, such as deleting the server. The client keeps no token: each
//! request Discord holds back asks again.

use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde_json::Value;
use tokio::sync::oneshot;

/// Discord's code for a request that needs two-factor authentication.
pub const MFA_REQUIRED: u64 = 60003;
/// Discord's code for a code, password or backup code it didn't take.
pub const INVALID_MFA: u64 = 60008;

/// A way to confirm, as the ticket offers them, in the order Discord gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// A passkey or security key, which needs a browser; Yapfast can't.
    Webauthn,
    /// A code from an authenticator app.
    Totp,
    /// A code Discord texts, once asked to.
    Sms,
    Password,
    Backup,
}

impl Method {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "webauthn" => Self::Webauthn,
            "totp" => Self::Totp,
            "sms" => Self::Sms,
            "password" => Self::Password,
            "backup" => Self::Backup,
            _ => return None,
        })
    }

    /// The method's name in `POST /mfa/finish`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Webauthn => "webauthn",
            Self::Totp => "totp",
            Self::Sms => "sms",
            Self::Password => "password",
            Self::Backup => "backup",
        }
    }
}

/// Discord held a request back until it's confirmed: the error a refused
/// request carries at its root, for [`crate::service::Discord::stepping_up`].
#[derive(Clone, Debug)]
pub struct Required {
    pub ticket: String,
    pub methods: Vec<Method>,
}

impl Required {
    /// The ticket in a 401's body, as `{"code": 60003, "mfa": {...}}`
    /// carries it; methods Yapfast doesn't know are dropped, as Discord's
    /// client drops them.
    pub(crate) fn from_body(body: &Value) -> Option<Self> {
        let mfa = &body["mfa"];
        let ticket = mfa["ticket"].as_str()?.to_owned();
        let methods = mfa["methods"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|method| Method::from_name(method["type"].as_str()?))
            .collect();
        Some(Self { ticket, methods })
    }
}

impl std::fmt::Display for Required {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Discord asks to confirm with two-factor authentication.")
    }
}

impl std::error::Error for Required {}

tokio::task_local! {
    /// The token `POST /mfa/finish` gave, for the requests a confirmed
    /// action sends again.
    static TOKEN: String;
}

/// Runs `work` with its requests carrying `token`.
pub(crate) async fn with_token<F: Future>(token: String, work: F) -> F::Output {
    TOKEN.scope(token, work).await
}

/// The token the running request carries, if it's sent again after
/// confirming.
pub(crate) fn token() -> Option<String> {
    TOKEN.try_with(Clone::clone).ok()
}

/// What the person did with a prompt.
pub(crate) enum Answer {
    /// Confirmed: the token to send the request again with.
    Token(String),
    Cancelled,
}

/// The prompts waiting for the person, by id: the ticket each confirms, and
/// the request waiting on it.
#[derive(Default)]
pub(crate) struct Prompts {
    next: AtomicU64,
    waiting: Mutex<HashMap<u64, Waiting>>,
}

struct Waiting {
    ticket: String,
    answer: oneshot::Sender<Answer>,
}

impl Prompts {
    /// Waits on a new prompt for `ticket`, by its id.
    pub(crate) fn ask(&self, ticket: String) -> (u64, oneshot::Receiver<Answer>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (answer, answered) = oneshot::channel();
        self.lock().insert(id, Waiting { ticket, answer });
        (id, answered)
    }

    /// The ticket of a prompt still waiting.
    pub(crate) fn ticket(&self, id: u64) -> Option<String> {
        self.lock().get(&id).map(|waiting| waiting.ticket.clone())
    }

    /// Ends a prompt with the person's answer.
    pub(crate) fn answer(&self, id: u64, answer: Answer) {
        if let Some(waiting) = self.lock().remove(&id) {
            let _ = waiting.answer.send(answer);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Waiting>> {
        self.waiting
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_ticket_keeps_the_methods_yapfast_knows_in_discords_order() {
        let body = json!({"code": 60003, "mfa": {"ticket": "t", "methods": [
            {"type": "webauthn", "challenge": "{}"},
            {"type": "totp", "backup_codes_allowed": true},
            {"type": "carrier-pigeon"},
            {"type": "backup"},
        ]}});
        let required = Required::from_body(&body).unwrap();
        assert_eq!(required.ticket, "t");
        assert_eq!(
            required.methods,
            [Method::Webauthn, Method::Totp, Method::Backup]
        );
        assert!(Required::from_body(&json!({"code": 60003})).is_none());
    }

    #[tokio::test]
    async fn a_prompt_ends_once_with_its_answer() {
        let prompts = Prompts::default();
        let (id, answered) = prompts.ask("t".into());
        assert_eq!(prompts.ticket(id).as_deref(), Some("t"));
        prompts.answer(id, Answer::Token("token".into()));
        prompts.answer(id, Answer::Cancelled);
        assert!(matches!(answered.await, Ok(Answer::Token(token)) if token == "token"));
        assert!(prompts.ticket(id).is_none());
    }

    #[tokio::test]
    async fn requests_carry_the_token_only_inside_its_scope() {
        assert_eq!(token(), None);
        let inside = with_token("token".into(), async { token() }).await;
        assert_eq!(inside.as_deref(), Some("token"));
    }
}
