use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRequestContext {
    pub os_version: String,
    pub shell: Option<String>,
    pub time_zone: Option<String>,
    pub transcripts_folder: PathBuf,
    pub user_full_name: Option<String>,
}

pub fn normalize_sand_user_full_name(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }

    let bounded: String = value.chars().take(160).collect();
    if bounded.chars().any(char::is_control) {
        return None;
    }
    Some(bounded)
}

pub fn resolve_time_zone_from_environment() -> Option<String> {
    std::env::var("TZ")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub struct HostRequestContextResolver<TimeZone, FullName, Rules> {
    transcripts_folder: PathBuf,
    resolve_user_time_zone: TimeZone,
    resolve_user_full_name: FullName,
    resolve_rules: Rules,
}

impl<TimeZone, FullName, Rules> HostRequestContextResolver<TimeZone, FullName, Rules>
where
    TimeZone: Fn() -> Option<String>,
    FullName: Fn() -> Option<String>,
{
    pub fn new(
        transcripts_folder: impl Into<PathBuf>,
        resolve_user_time_zone: TimeZone,
        resolve_user_full_name: FullName,
        resolve_rules: Rules,
    ) -> Self {
        Self {
            transcripts_folder: transcripts_folder.into(),
            resolve_user_time_zone,
            resolve_user_full_name,
            resolve_rules,
        }
    }

    pub fn resolve(&self) -> HostRequestContext {
        let supplied_zone = (self.resolve_user_time_zone)()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        HostRequestContext {
            os_version: format!(
                "{} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
            shell: std::env::var("SHELL")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            time_zone: supplied_zone.or_else(resolve_time_zone_from_environment),
            transcripts_folder: self.transcripts_folder.clone(),
            user_full_name: normalize_sand_user_full_name(
                (self.resolve_user_full_name)().as_deref(),
            ),
        }
    }

    pub fn transcripts_folder(&self) -> &Path {
        &self.transcripts_folder
    }
}

impl<TimeZone, FullName, Rules>
    HostRequestContextResolver<TimeZone, FullName, Rules>
{
    pub fn resolve_rules<Rule, Error>(&self) -> Result<Vec<Rule>, Error>
    where
        Rules: Fn() -> Result<Vec<Rule>, Error>,
    {
        (self.resolve_rules)()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_platform_context_without_exposing_empty_identity() {
        let resolver = HostRequestContextResolver::new(
            "/tmp/transcripts",
            || Some("America/Los_Angeles".into()),
            || Some("  Alice Example  ".into()),
            || Ok::<_, ()>(vec!["rule"]),
        );

        let context = resolver.resolve();
        assert_eq!(context.time_zone.as_deref(), Some("America/Los_Angeles"));
        assert_eq!(context.user_full_name.as_deref(), Some("Alice Example"));
        assert_eq!(context.transcripts_folder, PathBuf::from("/tmp/transcripts"));
        assert!(context.os_version.contains(std::env::consts::OS));
        assert_eq!(resolver.resolve_rules().unwrap(), vec!["rule"]);
    }

    #[test]
    fn rejects_control_characters_and_empty_names() {
        assert_eq!(normalize_sand_user_full_name(Some("   ")), None);
        assert_eq!(normalize_sand_user_full_name(Some("bad\nname")), None);
    }
}
