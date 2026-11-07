use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde_json::Value;

use crate::extensions::auth::credential_renewer::RenewalOutcome;
use crate::extensions::auth::extension::HostAuthExtension;

use super::cursor_skills_marketplace::SkillCatalogEntry;
use super::production::ProductionManagedSetup;
use super::team_rules::ProductionTeamRulesResolver;

pub struct ManagedSetupExtension {
    auth: Arc<HostAuthExtension>,
    production: Arc<ProductionManagedSetup>,
    team_rules: Arc<ProductionTeamRulesResolver>,
    renewal_subscription: Option<u64>,
}

impl ManagedSetupExtension {
    pub fn ensure_managed_skill(&self, id: &str) -> bool {
        self.production.managed_skills().ensure_skill(id)
    }

    pub fn skills_catalog(&self) -> Vec<SkillCatalogEntry> {
        self.production.fetch_skill_catalog()
    }

    pub fn resolve_team_rules(&self) -> Option<Vec<Value>> {
        self.team_rules.resolve_rules()
    }

    pub fn team_rules(&self) -> &Arc<ProductionTeamRulesResolver> {
        &self.team_rules
    }

    pub fn managed_skills(&self) -> &Arc<super::managed_skills_service::SandManagedSkillsService> {
        self.production.managed_skills()
    }
}

impl Drop for ManagedSetupExtension {
    fn drop(&mut self) {
        if let Some(subscription) = self.renewal_subscription.take() {
            self.auth.service().unsubscribe_from_renewal(subscription);
        }
        self.production.managed_skills().dispose();
    }
}

pub fn start_managed_setup_extension(
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    sand_root: impl AsRef<Path>,
) -> Arc<ManagedSetupExtension> {
    let production = Arc::new(ProductionManagedSetup::new(
        backend_url.clone(),
        Arc::clone(&auth),
        sand_root,
    ));
    let team_rules = Arc::new(ProductionTeamRulesResolver::new(
        backend_url,
        Arc::clone(&auth),
    ));
    team_rules.preload();

    let managed_started = Arc::new(AtomicBool::new(false));
    if auth.peek_access_token().is_some() {
        production.managed_skills().start();
        managed_started.store(true, Ordering::Release);
    }

    let weak_production = Arc::downgrade(&production);
    let weak_team_rules = Arc::downgrade(&team_rules);
    let started_for_renewal = Arc::clone(&managed_started);
    let subscription = auth.service().subscribe_to_renewal(Arc::new(move |event| {
        if event.result.outcome != RenewalOutcome::Renewed {
            return;
        }

        if event.is_first_credential {
            if let Some(team_rules) = weak_team_rules.upgrade() {
                let _ = std::thread::Builder::new()
                    .name("host-managed-team-rules-first-credential".into())
                    .spawn(move || {
                        let _ = team_rules.refresh();
                    });
            }
        }

        let Some(production) = weak_production.upgrade() else {
            return;
        };
        if !started_for_renewal.swap(true, Ordering::AcqRel) {
            production.managed_skills().start();
        } else if event.is_first_credential {
            production.managed_skills().handle_auth_change();
        }
    }));

    Arc::new(ManagedSetupExtension {
        auth,
        production,
        team_rules,
        renewal_subscription: Some(subscription),
    })
}
