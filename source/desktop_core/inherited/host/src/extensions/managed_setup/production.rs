use std::path::Path;
use std::sync::Arc;

use crate::extensions::auth::extension::HostAuthExtension;

use super::cursor_skills_marketplace::{
    SkillCatalogEntry, fetch_sand_managed_skills, fetch_skill_catalog,
};
use super::managed_skills_cache::get_managed_skills_dir;
use super::managed_skills_service::{
    ManagedSkillsDiagnostic, ManagedSkillsFetch, ManagedSkillsReport, SandManagedSkillsService,
};

pub struct ProductionManagedSetup {
    backend_url: String,
    auth: Arc<HostAuthExtension>,
    managed_skills: Arc<SandManagedSkillsService>,
}

impl ProductionManagedSetup {
    pub fn new(
        backend_url: String,
        auth: Arc<HostAuthExtension>,
        sand_root: impl AsRef<Path>,
    ) -> Self {
        let fetch_backend = backend_url.clone();
        let fetch_auth = Arc::clone(&auth);
        let fetch: ManagedSkillsFetch = Arc::new(move || {
            let access_token = fetch_auth
                .get_access_token()
                .map_err(|error| error.to_string())?;
            let machine_id = fetch_auth
                .get_machine_id()
                .map_err(|error| error.to_string())?;
            fetch_sand_managed_skills(
                &fetch_backend,
                &access_token,
                &machine_id,
            )
            .map_err(|error| error.to_string())
        });

        let report_auth = Arc::clone(&auth);
        let report: ManagedSkillsReport = Arc::new(move |event: ManagedSkillsDiagnostic| {
            report_auth.service().log(&format!(
                "{} {} refresh failed: {}",
                event.extension, event.kind, event.error_class
            ));
        });

        let cache_dir = get_managed_skills_dir(sand_root);
        let managed_skills = Arc::new(SandManagedSkillsService::new(
            cache_dir,
            fetch,
            Some(report),
        ));

        Self {
            backend_url,
            auth,
            managed_skills,
        }
    }

    pub fn managed_skills(&self) -> &Arc<SandManagedSkillsService> {
        &self.managed_skills
    }

    pub fn fetch_skill_catalog(&self) -> Vec<SkillCatalogEntry> {
        let result = (|| {
            let access_token = self
                .auth
                .get_access_token()
                .map_err(|error| error.to_string())?;
            let machine_id = self
                .auth
                .get_machine_id()
                .map_err(|error| error.to_string())?;
            fetch_skill_catalog(
                &self.backend_url,
                &access_token,
                &machine_id,
            )
            .map_err(|error| error.to_string())
        })();

        match result {
            Ok(entries) => entries,
            Err(error) => {
                self.auth
                    .service()
                    .log(&format!("managed_setup skill_catalog failed: {error}"));
                Vec::new()
            }
        }
    }
}
