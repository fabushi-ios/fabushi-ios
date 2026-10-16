#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandMessageLengthBucket {
    Empty,
    Xs,
    S,
    M,
    L,
    Xl,
}

impl SandMessageLengthBucket {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Xs => "xs",
            Self::S => "s",
            Self::M => "m",
            Self::L => "l",
            Self::Xl => "xl",
        }
    }
}

pub fn sand_message_length_bucket(length: i64) -> SandMessageLengthBucket {
    if length <= 0 { SandMessageLengthBucket::Empty }
    else if length < 20 { SandMessageLengthBucket::Xs }
    else if length < 100 { SandMessageLengthBucket::S }
    else if length < 500 { SandMessageLengthBucket::M }
    else if length < 2_000 { SandMessageLengthBucket::L }
    else { SandMessageLengthBucket::Xl }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matches_reference_bucket_boundaries() {
        let cases = [
            (-1, "empty"), (0, "empty"), (1, "xs"), (19, "xs"), (20, "s"),
            (99, "s"), (100, "m"), (499, "m"), (500, "l"), (1_999, "l"), (2_000, "xl")
        ];
        for (length, expected) in cases {
            assert_eq!(sand_message_length_bucket(length).as_str(), expected);
        }
    }
}
