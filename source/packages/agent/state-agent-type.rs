#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    Ide,
    Cli,
    Background,
    Bugbot,
}

impl AgentType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ide => "ide",
            Self::Cli => "cli",
            Self::Background => "background",
            Self::Bugbot => "bugbot",
        }
    }
}

/// Parses only the four recovered AgentType wire values.
/// Unknown or absent values intentionally fail closed to None.
pub fn parse_agent_type(value: Option<&str>) -> Option<AgentType> {
    match value {
        Some("ide") => Some(AgentType::Ide),
        Some("cli") => Some(AgentType::Cli),
        Some("background") => Some(AgentType::Background),
        Some("bugbot") => Some(AgentType::Bugbot),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_recovered_agent_type_values() {
        for (wire, expected) in [
            ("ide", AgentType::Ide),
            ("cli", AgentType::Cli),
            ("background", AgentType::Background),
            ("bugbot", AgentType::Bugbot),
        ] {
            assert_eq!(parse_agent_type(Some(wire)), Some(expected));
            assert_eq!(expected.as_str(), wire);
        }
        assert_eq!(parse_agent_type(None), None);
        assert_eq!(parse_agent_type(Some("IDE")), None);
        assert_eq!(parse_agent_type(Some("unknown")), None);
    }
}
