use std::future::Future;
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeadlineExceededError {
    pub policy_name: String,
}

impl std::fmt::Display for DeadlineExceededError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Deadline exceeded for {}", self.policy_name)
    }
}

impl std::error::Error for DeadlineExceededError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeadlinePolicy {
    pub name: String,
    pub timeout: Duration,
}

impl DeadlinePolicy {
    pub fn new(name: impl Into<String>, timeout: Duration) -> Result<Self, &'static str> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("name must not be empty");
        }
        Ok(Self { name, timeout })
    }

    pub async fn run<T, F>(&self, work: F) -> Result<T, DeadlineExceededError>
    where
        F: Future<Output = T>,
    {
        if self.timeout.is_zero() {
            return Err(DeadlineExceededError {
                policy_name: self.name.clone(),
            });
        }

        let deadline = tokio::time::sleep(self.timeout);
        tokio::pin!(deadline);
        tokio::pin!(work);

        tokio::select! {
            biased;
            _ = &mut deadline => Err(DeadlineExceededError {
                policy_name: self.name.clone(),
            }),
            value = &mut work => Ok(value),
        }
    }
}

#[derive(Clone, Debug)]
pub struct RetryPolicy {
    pub name: String,
    pub max_attempts: usize,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub backoff_factor: f64,
}

impl RetryPolicy {
    pub fn new(
        name: impl Into<String>,
        max_attempts: usize,
        initial_delay: Duration,
        max_delay: Duration,
        backoff_factor: f64,
    ) -> Result<Self, &'static str> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err("name must not be empty");
        }
        if max_attempts == 0 {
            return Err("max_attempts must be positive");
        }
        if max_delay < initial_delay {
            return Err("max_delay must be at least initial_delay");
        }
        if !backoff_factor.is_finite() || backoff_factor < 1.0 {
            return Err("backoff_factor must be finite and at least 1");
        }
        Ok(Self {
            name,
            max_attempts,
            initial_delay,
            max_delay,
            backoff_factor,
        })
    }

    pub fn delay_for(&self, attempt: usize) -> Duration {
        let exponent = attempt.saturating_sub(1).min(i32::MAX as usize) as i32;
        let multiplier = self.backoff_factor.powi(exponent);
        let millis = (self.initial_delay.as_millis() as f64 * multiplier)
            .min(self.max_delay.as_millis() as f64)
            .max(0.0) as u64;
        Duration::from_millis(millis)
    }

    pub async fn run_with_retry<T, E, F, Fut, P>(
        &self,
        mut work: F,
        mut should_retry: P,
    ) -> Result<T, E>
    where
        F: FnMut(usize) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        P: FnMut(&E, usize) -> bool,
    {
        let mut attempt = 1usize;
        loop {
            match work(attempt).await {
                Ok(value) => return Ok(value),
                Err(error) if attempt < self.max_attempts && should_retry(&error, attempt) => {
                    tokio::time::sleep(self.delay_for(attempt)).await;
                    attempt += 1;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PollingPolicy {
    pub name: String,
    pub interval: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpiryPolicy {
    pub name: String,
    pub ttl: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdleWatchdogPolicy {
    pub name: String,
    pub idle: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebouncePolicy {
    pub name: String,
    pub delay: Duration,
}

fn named_duration(
    name: impl Into<String>,
    duration: Duration,
    require_non_zero: bool,
) -> Result<(String, Duration), &'static str> {
    let name = name.into();
    if name.trim().is_empty() {
        return Err("name must not be empty");
    }
    if require_non_zero && duration.is_zero() {
        return Err("duration must be greater than zero");
    }
    Ok((name, duration))
}

impl PollingPolicy {
    pub fn new(name: impl Into<String>, interval: Duration) -> Result<Self, &'static str> {
        let (name, interval) = named_duration(name, interval, true)?;
        Ok(Self { name, interval })
    }
}

impl ExpiryPolicy {
    pub fn new(name: impl Into<String>, ttl: Duration) -> Result<Self, &'static str> {
        let (name, ttl) = named_duration(name, ttl, false)?;
        Ok(Self { name, ttl })
    }
}

impl IdleWatchdogPolicy {
    pub fn new(name: impl Into<String>, idle: Duration) -> Result<Self, &'static str> {
        let (name, idle) = named_duration(name, idle, false)?;
        Ok(Self { name, idle })
    }
}

impl DebouncePolicy {
    pub fn new(name: impl Into<String>, delay: Duration) -> Result<Self, &'static str> {
        let (name, delay) = named_duration(name, delay, false)?;
        Ok(Self { name, delay })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn deadline_fails_closed() {
        let policy = DeadlinePolicy::new("passkey", Duration::from_millis(1)).unwrap();
        let result = policy
            .run(async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                7
            })
            .await;
        assert!(matches!(result, Err(DeadlineExceededError { .. })));
    }

    #[tokio::test]
    async fn zero_deadline_fails_closed_even_when_work_is_already_ready() {
        let policy = DeadlinePolicy::new("ready-work", Duration::ZERO).unwrap();
        let result = policy.run(async { 7 }).await;
        assert_eq!(
            result,
            Err(DeadlineExceededError {
                policy_name: "ready-work".into(),
            })
        );
    }

    #[test]
    fn retry_delay_is_bounded() {
        let policy = RetryPolicy::new(
            "gateway",
            4,
            Duration::from_millis(100),
            Duration::from_millis(250),
            2.0,
        )
        .unwrap();
        assert_eq!(policy.delay_for(1), Duration::from_millis(100));
        assert_eq!(policy.delay_for(2), Duration::from_millis(200));
        assert_eq!(policy.delay_for(3), Duration::from_millis(250));
    }
}
