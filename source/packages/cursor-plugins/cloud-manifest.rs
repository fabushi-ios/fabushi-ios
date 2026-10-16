#[derive(Debug, Clone, Copy)]
pub struct ReleasePluginFields<'a> {
    pub release_tag: Option<&'a str>,
    pub release_repo: Option<&'a str>,
    pub release_asset: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleasePluginSource {
    pub release_repo: String,
    pub release_asset: String,
    pub release_tag: String,
}

pub fn get_release_plugin_source(
    fields: ReleasePluginFields<'_>,
    fallback_release_tag: Option<&str>,
) -> Option<ReleasePluginSource> {
    let release_repo = fields.release_repo?;
    let release_asset = fields.release_asset?;
    let release_tag = fields.release_tag.or(fallback_release_tag)?;

    if release_repo.is_empty() || release_asset.is_empty() || release_tag.is_empty() {
        return None;
    }

    Some(ReleasePluginSource {
        release_repo: release_repo.to_owned(),
        release_asset: release_asset.to_owned(),
        release_tag: release_tag.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_truthy_repo_asset_and_resolved_release_tag() {
        let fields = ReleasePluginFields {
            release_tag: None,
            release_repo: Some("owner/repo"),
            release_asset: Some("plugin.tgz"),
        };
        assert_eq!(
            get_release_plugin_source(fields, Some("v1.2.3")),
            Some(ReleasePluginSource {
                release_repo: "owner/repo".to_owned(),
                release_asset: "plugin.tgz".to_owned(),
                release_tag: "v1.2.3".to_owned(),
            })
        );

        assert_eq!(
            get_release_plugin_source(
                ReleasePluginFields {
                    release_tag: Some(""),
                    release_repo: Some("owner/repo"),
                    release_asset: Some("plugin.tgz"),
                },
                Some("fallback")
            ),
            None
        );
        assert_eq!(
            get_release_plugin_source(
                ReleasePluginFields {
                    release_tag: Some("v1"),
                    release_repo: None,
                    release_asset: Some("plugin.tgz"),
                },
                None
            ),
            None
        );
    }
}
