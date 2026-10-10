pub trait EnvironmentScoped {
    fn disabled_environments(&self) -> &[String];
    fn environments(&self) -> &[String];
}

pub fn is_environment_eligible<T: EnvironmentScoped>(item: &T, target_env: &str) -> bool {
    if item.disabled_environments().iter().any(|env| env == target_env) {
        return false;
    }
    let environments = item.environments();
    environments.is_empty() || environments.iter().any(|env| env == target_env)
}

pub fn filter_by_environment<T: EnvironmentScoped + Clone>(items: &[T], target_env: &str) -> Vec<T> {
    items
        .iter()
        .filter(|item| is_environment_eligible(*item, target_env))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Item {
        id: &'static str,
        disabled: Vec<String>,
        enabled: Vec<String>,
    }

    impl EnvironmentScoped for Item {
        fn disabled_environments(&self) -> &[String] { &self.disabled }
        fn environments(&self) -> &[String] { &self.enabled }
    }

    #[test]
    fn disabled_environment_wins_and_empty_allowlist_is_global() {
        let items = vec![
            Item { id: "global", disabled: vec![], enabled: vec![] },
            Item { id: "prod", disabled: vec![], enabled: vec!["prod".into()] },
            Item { id: "blocked", disabled: vec!["prod".into()], enabled: vec!["prod".into()] },
            Item { id: "dev", disabled: vec![], enabled: vec!["dev".into()] },
        ];
        let ids: Vec<_> = filter_by_environment(&items, "prod").into_iter().map(|item| item.id).collect();
        assert_eq!(ids, vec!["global", "prod"]);
    }
}
