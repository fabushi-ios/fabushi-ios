use std::collections::HashMap;
use std::hash::Hash;
use std::time::{SystemTime, UNIX_EPOCH};

struct Entry<V> {
    value: V,
    expires_at_ms: f64,
}

pub struct TtlCache<K, V> {
    entries: HashMap<K, Entry<V>>,
    ttl_ms: Box<dyn Fn() -> f64>,
    now: Box<dyn Fn() -> f64>,
}

impl<K, V> TtlCache<K, V>
where
    K: Eq + Hash,
{
    pub fn new(ttl_ms: f64) -> Result<Self, String> {
        if !ttl_ms.is_finite() || ttl_ms <= 0.0 {
            return Err(format!("TTL cache requires a positive ttlMs, got {ttl_ms}"));
        }
        Ok(Self::with_sources(move || ttl_ms, system_now_ms))
    }

    pub fn with_sources<Ttl, Now>(ttl_ms: Ttl, now: Now) -> Self
    where
        Ttl: Fn() -> f64 + 'static,
        Now: Fn() -> f64 + 'static,
    {
        Self {
            entries: HashMap::new(),
            ttl_ms: Box::new(ttl_ms),
            now: Box::new(now),
        }
    }

    pub fn get(&mut self, key: &K) -> Option<&V> {
        let expired = self
            .entries
            .get(key)
            .is_some_and(|entry| entry.expires_at_ms <= (self.now)());
        if expired {
            self.entries.remove(key);
            return None;
        }
        self.entries.get(key).map(|entry| &entry.value)
    }

    pub fn has(&mut self, key: &K) -> bool {
        self.get(key).is_some()
    }

    pub fn set(&mut self, key: K, value: V) {
        let expires_at_ms = (self.now)() + (self.ttl_ms)();
        self.entries.insert(
            key,
            Entry {
                value,
                expires_at_ms,
            },
        );
    }

    pub fn delete(&mut self, key: &K) -> bool {
        self.entries.remove(key).is_some()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

fn system_now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn fixed_ttl_must_be_positive_and_finite() {
        assert!(TtlCache::<String, i32>::new(1.0).is_ok());
        assert!(TtlCache::<String, i32>::new(0.0).is_err());
        assert!(TtlCache::<String, i32>::new(f64::NAN).is_err());
    }

    #[test]
    fn expires_on_get_and_has_and_supports_delete_and_clear() {
        let now = Rc::new(Cell::new(100.0));
        let clock = {
            let now = Rc::clone(&now);
            move || now.get()
        };
        let mut cache = TtlCache::with_sources(|| 50.0, clock);
        cache.set("a", 1);
        assert_eq!(cache.get(&"a"), Some(&1));
        now.set(150.0);
        assert!(!cache.has(&"a"));

        cache.set("b", 2);
        assert!(cache.delete(&"b"));
        cache.set("c", 3);
        cache.clear();
        assert!(!cache.has(&"c"));
    }

    #[test]
    fn dynamic_ttl_is_read_at_set_time_like_the_reference() {
        let now = Rc::new(Cell::new(0.0));
        let ttl = Rc::new(Cell::new(10.0));
        let mut cache = TtlCache::with_sources(
            {
                let ttl = Rc::clone(&ttl);
                move || ttl.get()
            },
            {
                let now = Rc::clone(&now);
                move || now.get()
            },
        );
        cache.set("first", 1);
        ttl.set(100.0);
        cache.set("second", 2);
        now.set(11.0);
        assert!(!cache.has(&"first"));
        assert!(cache.has(&"second"));
    }
}
