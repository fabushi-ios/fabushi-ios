pub const DEFAULT_SAND_COMPUTER_ID: &str = "this-computer";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserComputerDescriptor {
    pub id: String,
    pub label: String,
    pub connected: bool,
}

#[derive(Debug)]
pub struct ResolvedUserComputer<'a, BoxT> {
    pub id: &'static str,
    pub label: &'a str,
    pub box_handle: &'a BoxT,
}

pub struct SingleUserComputer<BoxT, Connected = fn() -> bool> {
    box_handle: BoxT,
    label: String,
    is_connected: Connected,
}

fn always_connected() -> bool { true }

impl<BoxT> SingleUserComputer<BoxT, fn() -> bool> {
    pub fn new(box_handle: BoxT, label: Option<String>) -> Self {
        Self {
            box_handle,
            label: label.unwrap_or_else(|| "this computer".into()),
            is_connected: always_connected,
        }
    }
}

impl<BoxT, Connected> SingleUserComputer<BoxT, Connected>
where
    Connected: Fn() -> bool,
{
    pub fn with_connected(
        box_handle: BoxT,
        label: Option<String>,
        is_connected: Connected,
    ) -> Self {
        Self {
            box_handle,
            label: label.unwrap_or_else(|| "this computer".into()),
            is_connected,
        }
    }

    pub fn list(&self) -> Vec<UserComputerDescriptor> {
        vec![UserComputerDescriptor {
            id: DEFAULT_SAND_COMPUTER_ID.into(),
            label: self.label.clone(),
            connected: (self.is_connected)(),
        }]
    }

    pub fn resolve(&self, requested_id: Option<&str>) -> Option<ResolvedUserComputer<'_, BoxT>> {
        if requested_id.is_some_and(|id| id != DEFAULT_SAND_COMPUTER_ID)
            || !(self.is_connected)()
        {
            return None;
        }
        Some(ResolvedUserComputer {
            id: DEFAULT_SAND_COMPUTER_ID,
            label: &self.label,
            box_handle: &self.box_handle,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_default_or_explicit_local_computer_only_when_connected() {
        let computer = SingleUserComputer::new(42u8, None);
        assert_eq!(computer.list()[0].label, "this computer");
        assert_eq!(*computer.resolve(None).unwrap().box_handle, 42);
        assert!(computer.resolve(Some("other")).is_none());
        let disconnected = SingleUserComputer::with_connected(7u8, None, || false);
        assert!(disconnected.resolve(None).is_none());
    }
}
