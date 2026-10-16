pub const INT32_MIN: f64 = i32::MIN as f64;
pub const INT32_MAX: f64 = i32::MAX as f64;

pub fn clamp_int32(value: f64) -> f64 {
    if value.is_nan() {
        return 0.0;
    }
    value.clamp(INT32_MIN, INT32_MAX)
}

pub fn to_optional_duration_ms_int32(value: Option<f64>) -> Option<i32> {
    let value = value?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    Some(clamp_int32(value.trunc()) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_matches_javascript_number_contract() {
        assert_eq!(clamp_int32(f64::NAN), 0.0);
        assert_eq!(clamp_int32(-3_000_000_000.0), i32::MIN as f64);
        assert_eq!(clamp_int32(3_000_000_000.0), i32::MAX as f64);
        assert_eq!(clamp_int32(12.75), 12.75);
    }

    #[test]
    fn optional_duration_rejects_invalid_and_truncates_valid_values() {
        assert_eq!(to_optional_duration_ms_int32(None), None);
        assert_eq!(to_optional_duration_ms_int32(Some(-1.0)), None);
        assert_eq!(to_optional_duration_ms_int32(Some(f64::INFINITY)), None);
        assert_eq!(to_optional_duration_ms_int32(Some(12.9)), Some(12));
        assert_eq!(
            to_optional_duration_ms_int32(Some(3_000_000_000.0)),
            Some(i32::MAX)
        );
    }
}
