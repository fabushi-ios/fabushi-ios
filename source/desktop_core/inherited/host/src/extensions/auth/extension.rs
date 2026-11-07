use std::sync::Arc;
use std::thread;

use crate::extensions::browser_ua::extension::{
    BrowserUaAuthApi, BrowserUaAuthRenewalEvent, StopSubscription,
};
use crate::extensions::extension_ids_generated::HostExtensionId;

use super::auth_service::{
    HostAuthService, HostAuthServiceOptions, SandCredentialsWaitingError,
};
use super::credential_renewer::{
    RenewalOutcome, SandCredentialRenewalError,
};
use super::user_full_name_service::{
    SandUserFullNameResolver, UserFullNameFetch, UserFullNameLog,
};

pub const AUTH_DEPENDENCIES: &[HostExtensionId] = &[];

pub fn auth_extension_id() -> HostExtensionId {
    HostExtensionId::Auth
}

pub struct HostAuthExtension {
    service: Arc<HostAuthService>,
    user_full_name: Arc<SandUserFullNameResolver>,
    renewal_subscription: Option<u64>,
}

impl HostAuthExtension {
    pub fn get_access_token(&self) -> Result<String, SandCredentialsWaitingError> {
        self.service.get_access_token()
    }

    pub fn peek_access_token(&self) -> Option<String> {
        self.service.peek_access_token()
    }

    pub fn get_machine_id(&self) -> Result<String, std::io::Error> {
        self.service.get_machine_id()
    }

    pub fn get_user_full_name(&self) -> Option<String> {
        self.user_full_name.get_user_full_name()
    }

    pub fn service(&self) -> &Arc<HostAuthService> {
        &self.service
    }

    pub fn stop(mut self) {
        self.dispose();
    }

    fn dispose(&mut self) {
        if let Some(subscription) = self.renewal_subscription.take() {
            self.service.unsubscribe_from_renewal(subscription);
        }
        self.service.dispose();
    }
}

impl Drop for HostAuthExtension {
    fn drop(&mut self) {
        self.dispose();
    }
}

impl BrowserUaAuthApi for HostAuthExtension {
    fn peek_access_token(&self) -> Option<String> {
        HostAuthExtension::peek_access_token(self)
    }

    fn subscribe_to_renewal(
        &self,
        listener: Arc<dyn Fn(BrowserUaAuthRenewalEvent) + Send + Sync>,
    ) -> StopSubscription {
        let id = self.service.subscribe_to_renewal(Arc::new(move |event| {
            listener(BrowserUaAuthRenewalEvent {
                outcome: match event.result.outcome {
                    RenewalOutcome::Renewed => "renewed",
                    RenewalOutcome::Failed => "failed",
                }
                .to_string(),
            });
        }));
        let service = Arc::clone(&self.service);
        Box::new(move || {
            service.unsubscribe_from_renewal(id);
        })
    }
}

pub fn start_host_auth_extension_with_options(
    options: HostAuthServiceOptions,
    fetch_full_name: UserFullNameFetch,
) -> Result<HostAuthExtension, SandCredentialRenewalError> {
    let log: UserFullNameLog = Arc::clone(&options.log);
    let service = Arc::new(HostAuthService::new(options)?);
    let get_service = Arc::clone(&service);
    let peek_service = Arc::clone(&service);
    let resolver = Arc::new(SandUserFullNameResolver::new(
        Arc::new(move || get_service.get_access_token().map_err(|error| error.to_string())),
        Arc::new(move || peek_service.peek_access_token()),
        fetch_full_name,
        log,
    ));

    let renewal_resolver = Arc::clone(&resolver);
    let renewal_subscription = service.subscribe_to_renewal(Arc::new(move |event| {
        if event.result.outcome == RenewalOutcome::Renewed {
            let resolver = Arc::clone(&renewal_resolver);
            let _ = thread::Builder::new()
                .name("host-auth-user-full-name".into())
                .spawn(move || resolver.refresh());
        }
    }));

    if service.peek_access_token().is_some() {
        let resolver = Arc::clone(&resolver);
        let _ = thread::Builder::new()
            .name("host-auth-user-full-name-initial".into())
            .spawn(move || resolver.refresh());
    }

    Ok(HostAuthExtension {
        service,
        user_full_name: resolver,
        renewal_subscription: Some(renewal_subscription),
    })
}
