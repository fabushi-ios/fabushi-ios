pub fn ensure_finite_positive(value: f64, name: &str, context: &str) -> Result<f64, String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!(
            "{context} {name} must be a finite positive number, got {value}"
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::ensure_finite_positive;

    #[test]
    fn accepts_finite_positive_values_unchanged() {
        assert_eq!(
            ensure_finite_positive(250.5, "baseDelayMs", "BackoffScheduler"),
            Ok(250.5)
        );
    }

    #[test]
    fn rejects_zero_negative_nan_and_infinity() {
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let error = ensure_finite_positive(value, "delay", "sync").unwrap_err();
            assert!(error.starts_with("sync delay must be a finite positive number, got "));
        }
    }
}
