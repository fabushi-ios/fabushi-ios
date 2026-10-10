#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SamplingDecision {
    NotRecord = 0,
    Record = 1,
    RecordAndSampled = 2,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct AlwaysOffSampler;

impl AlwaysOffSampler {
    pub fn should_sample(&self) -> SamplingDecision {
        SamplingDecision::NotRecord
    }
}

impl std::fmt::Display for AlwaysOffSampler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AlwaysOffSampler")
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ParentBasedSampler {
    pub root: AlwaysOffSampler,
}

impl ParentBasedSampler {
    pub fn should_sample(&self) -> SamplingDecision {
        self.root.should_sample()
    }
}

impl std::fmt::Display for ParentBasedSampler {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "ParentBased{{root={}}}", self.root)
    }
}

pub fn create_send_trace_sampler() -> ParentBasedSampler {
    ParentBasedSampler::default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn send_traces_are_always_off() {
        let sampler = create_send_trace_sampler();
        assert_eq!(sampler.should_sample(), SamplingDecision::NotRecord);
        assert_eq!(sampler.to_string(), "ParentBased{root=AlwaysOffSampler}");
    }
}
