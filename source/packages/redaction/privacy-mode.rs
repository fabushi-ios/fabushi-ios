#[repr(i32)]
#[derive(Debug,Clone,Copy,PartialEq,Eq,Hash)]
pub enum PrivacyMode {
    Unspecified = 0,
    NoStorage = 1,
    NoTraining = 2,
    UsageDataTrainingAllowed = 3,
    UsageCodebaseTrainingAllowed = 4,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_pinned_numeric_wire_values() {
        assert_eq!(PrivacyMode::Unspecified as i32,0);
        assert_eq!(PrivacyMode::NoStorage as i32,1);
        assert_eq!(PrivacyMode::NoTraining as i32,2);
        assert_eq!(PrivacyMode::UsageDataTrainingAllowed as i32,3);
        assert_eq!(PrivacyMode::UsageCodebaseTrainingAllowed as i32,4);
    }
}
