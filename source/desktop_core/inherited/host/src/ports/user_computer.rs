pub const DEFAULT_SAND_COMPUTER_ID: &str = "this-computer";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserComputerSummary {
    pub id: &'static str,
    pub label: String,
    pub connected: bool,
}

pub struct ResolvedUserComputer<'a, B> {
    pub id: &'static str,
    pub label: &'a str,
    pub computer: &'a B,
}

pub struct SingleUserComputer<B, F = fn() -> bool>
where
    F: Fn() -> bool,
{
    computer: B,
    label: String,
    is_connected: F,
}

impl<B> SingleUserComputer<B, fn() -> bool> {
    pub fn always_connected(computer: B, label: Option<String>) -> Self {
        Self { computer, label: label.unwrap_or_else(|| "this computer".into()), is_connected: || true }
    }
}

impl<B, F> SingleUserComputer<B, F>
where
    F: Fn() -> bool,
{
    pub fn new(computer: B, label: Option<String>, is_connected: F) -> Self {
        Self { computer, label: label.unwrap_or_else(|| "this computer".into()), is_connected }
    }

    pub fn list(&self) -> [UserComputerSummary; 1] {
        [UserComputerSummary { id: DEFAULT_SAND_COMPUTER_ID, label: self.label.clone(), connected: (self.is_connected)() }]
    }

    pub fn resolve(&self, requested_id: Option<&str>) -> Option<ResolvedUserComputer<'_, B>> {
        if requested_id.is_some_and(|id| id != DEFAULT_SAND_COMPUTER_ID) || !(self.is_connected)() {
            return None;
        }
        Some(ResolvedUserComputer { id: DEFAULT_SAND_COMPUTER_ID, label: &self.label, computer: &self.computer })
    }
}
