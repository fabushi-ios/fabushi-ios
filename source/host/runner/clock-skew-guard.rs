pub const SEND_DISPATCH_MAX_PLAUSIBLE_MS: f64 = 120_000.0;
pub const TTFT_MAX_PLAUSIBLE_MS: f64 = 1_800_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockSkewReason {
    Negative,
    TooLarge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanitizedCrossClockDuration {
    Milliseconds(i64),
    Skew(ClockSkewReason),
}

pub fn sanitize_cross_clock_duration_ms(
    raw_delta_ms: f64,
    ceiling_ms: f64,
) -> SanitizedCrossClockDuration {
    if !raw_delta_ms.is_finite() {
        return SanitizedCrossClockDuration::Skew(ClockSkewReason::TooLarge);
    }
    if raw_delta_ms < 0.0 {
        return SanitizedCrossClockDuration::Skew(ClockSkewReason::Negative);
    }
    if raw_delta_ms > ceiling_ms {
        return SanitizedCrossClockDuration::Skew(ClockSkewReason::TooLarge);
    }
    SanitizedCrossClockDuration::Milliseconds(raw_delta_ms.round() as i64)
}

pub fn bucket_clock_skew_delta_ms(raw: f64) -> &'static str {
    if !raw.is_finite() { return "nonfinite"; }
    if raw < 0.0 {
        let n = -raw;
        if n <= 1_000.0 { "neg_le_1s" }
        else if n <= 60_000.0 { "neg_le_1m" }
        else { "neg_gt_1m" }
    } else if raw <= 60_000.0 { "le_1m" }
    else if raw <= 300_000.0 { "le_5m" }
    else if raw <= 900_000.0 { "le_15m" }
    else { "gt_15m" }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sanitizes_and_buckets_reference_boundaries() {
        assert_eq!(
            sanitize_cross_clock_duration_ms(-1.0, SEND_DISPATCH_MAX_PLAUSIBLE_MS),
            SanitizedCrossClockDuration::Skew(ClockSkewReason::Negative)
        );
        assert_eq!(
            sanitize_cross_clock_duration_ms(120_001.0, SEND_DISPATCH_MAX_PLAUSIBLE_MS),
            SanitizedCrossClockDuration::Skew(ClockSkewReason::TooLarge)
        );
        assert_eq!(
            sanitize_cross_clock_duration_ms(12.6, SEND_DISPATCH_MAX_PLAUSIBLE_MS),
            SanitizedCrossClockDuration::Milliseconds(13)
        );
        assert_eq!(bucket_clock_skew_delta_ms(-1_001.0), "neg_le_1m");
        assert_eq!(bucket_clock_skew_delta_ms(900_001.0), "gt_15m");
        assert_eq!(bucket_clock_skew_delta_ms(f64::INFINITY), "nonfinite");
    }
}
