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

pub fn sand_message_length_bucket(length: f64) -> SandMessageLengthBucket {
    if length <= 0.0 {
        SandMessageLengthBucket::Empty
    } else if length < 20.0 {
        SandMessageLengthBucket::Xs
    } else if length < 100.0 {
        SandMessageLengthBucket::S
    } else if length < 500.0 {
        SandMessageLengthBucket::M
    } else if length < 2_000.0 {
        SandMessageLengthBucket::L
    } else {
        SandMessageLengthBucket::Xl
    }
}
