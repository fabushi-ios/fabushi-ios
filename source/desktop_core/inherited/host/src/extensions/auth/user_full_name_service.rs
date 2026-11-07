use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;

use crate::cursor_backend::fetch_sand_user_full_name;
use crate::host_secret_store::get_or_create_host_machine_id;
use crate::sand_user_identity::normalize_sand_user_full_name;

pub const GET_ME_TIMEOUT_MS: u64 = 10_000;

pub type UserFullNameFetch =
    Arc<dyn Fn(&str) -> Result<Option<String>, String> + Send + Sync>;
pub type UserFullNameLog = Arc<dyn Fn(&str) + Send + Sync>;

pub fn production_user_full_name_fetch(backend_url: String) -> UserFullNameFetch {
    Arc::new(move |access_token| {
        let machine_id = get_or_create_host_machine_id(None)
            .map_err(|error| format!("could not resolve Host machine id for Dashboard GetMe: {error}"))?;
        fetch_sand_user_full_name(&backend_url, access_token, &machine_id)
            .map_err(|error| error.to_string())
    })
}

#[derive(Default)]
struct ResolverState {
    resolved_principal: Option<String>,
    resolved_full_name: Option<String>,
    refreshing_principal: Option<String>,
    generation: u64,
}

pub struct SandUserFullNameResolver {
    get_access_token: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
    peek_access_token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
    fetch_full_name: UserFullNameFetch,
    log: UserFullNameLog,
    state: Mutex<ResolverState>,
}

impl SandUserFullNameResolver {
    pub fn new(
        get_access_token: Arc<dyn Fn() -> Result<String, String> + Send + Sync>,
        peek_access_token: Arc<dyn Fn() -> Option<String> + Send + Sync>,
        fetch_full_name: UserFullNameFetch,
        log: UserFullNameLog,
    ) -> Self {
        Self {
            get_access_token,
            peek_access_token,
            fetch_full_name,
            log,
            state: Mutex::new(ResolverState::default()),
        }
    }

    fn current_principal(&self) -> Option<String> {
        (self.peek_access_token)()
            .as_deref()
            .and_then(principal_from_access_token)
    }

    pub fn get_user_full_name(&self) -> Option<String> {
        let principal = self.current_principal()?;
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (state.resolved_principal.as_deref() == Some(principal.as_str()))
            .then(|| state.resolved_full_name.clone())
            .flatten()
    }

    pub fn refresh(&self) {
        let Some(principal) = self.current_principal() else {
            return;
        };

        let generation = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.resolved_principal.as_deref() == Some(principal.as_str())
                || state.refreshing_principal.as_deref() == Some(principal.as_str())
            {
                return;
            }
            state.generation = state.generation.saturating_add(1);
            state.refreshing_principal = Some(principal.clone());
            state.generation
        };

        let result = (|| {
            let access_token = (self.get_access_token)()?;
            if principal_from_access_token(&access_token).as_deref() != Some(principal.as_str()) {
                return Ok(None);
            }
            let full_name = (self.fetch_full_name)(&access_token)?
                .and_then(|value| non_empty(Some(&value)));
            if self.current_principal().as_deref() != Some(principal.as_str()) {
                return Ok(None);
            }
            Ok::<Option<String>, String>(full_name)
        })();

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.generation != generation {
            return;
        }
        state.refreshing_principal = None;
        match result {
            Ok(full_name) => {
                state.resolved_principal = Some(principal);
                state.resolved_full_name = full_name;
            }
            Err(error) => {
                (self.log)(&format!("user full-name resolve failed: {error}"));
            }
        }
    }
}

pub fn non_empty(value: Option<&str>) -> Option<String> {
    let trimmed = value?.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

pub fn display_name_from(first_name: Option<&str>, last_name: Option<&str>) -> Option<String> {
    let parts = [first_name, last_name]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(" "))
}

pub fn principal_from_access_token(access_token: &str) -> Option<String> {
    let payload = access_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value = serde_json::from_slice::<Value>(&decoded).ok()?;
    normalize_sand_user_full_name(value.get("sub")?.as_str())
}
