use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::protocol::CollectorEvent;

#[derive(Debug)]
pub struct EventDeduplicator {
    ttl: Duration,
    seen: HashMap<CollectorEvent, Instant>,
}

impl EventDeduplicator {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            seen: HashMap::new(),
        }
    }

    pub fn accept(&mut self, event: &CollectorEvent, now: Instant) -> bool {
        self.seen
            .retain(|_, observed_at| now.duration_since(*observed_at) <= self.ttl);

        if self.seen.contains_key(event) {
            return false;
        }

        self.seen.insert(event.clone(), now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{CollectorEvent, GlobalEventType};

    #[test]
    fn rejects_same_event_inside_ttl_and_accepts_it_after_ttl() {
        let mut deduplicator = EventDeduplicator::new(Duration::from_secs(2));
        let now = Instant::now();
        let event = CollectorEvent::Global {
            event_type: GlobalEventType::Moon,
        };

        assert!(deduplicator.accept(&event, now));
        assert!(!deduplicator.accept(&event, now + Duration::from_millis(500)));
        assert!(deduplicator.accept(&event, now + Duration::from_secs(3)));
    }
}
