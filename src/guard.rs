//! Reservation around an in-flight send, so a retry never delivers twice.
//!
//! The server has no duplicate protection: submitting an Email twice sends it twice. So a
//! send reserves its key *before* its first await, under one lock, and holds it until the
//! call ends. Then:
//! - success records the result; a repeat of the same key gets that result back;
//! - a definite "no" (a JMAP error type) releases the key, so the call stays retryable;
//! - anything else (timeout, reset, 5xx, a dropped future) leaves the key held as "unknown":
//!   the message may have gone out, so a later attempt is refused until the entry expires.
//!
//! The lock is a `std::sync::Mutex` and is never held across an await, so the
//! check-and-reserve step is atomic on a multi-threaded runtime. Single process state: a
//! restart reopens the window. `spec/SendProtocol.tla` models this ordering.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const TTL: Duration = Duration::from_secs(10 * 60);

enum State {
    InFlight,
    Unknown,
    Done(Value),
}

#[derive(Clone, Default)]
pub struct Guard {
    map: Arc<Mutex<HashMap<String, (Instant, State)>>>,
}

#[derive(Debug, PartialEq)]
pub enum Refusal {
    InFlight,
    Unknown,
}

pub enum Begin {
    Go(Permit),
    Replay(Value),
}

impl Guard {
    /// Reserve `key`. Call before any await.
    pub fn begin(&self, key: &str) -> Result<Begin, Refusal> {
        let mut m = self.map.lock().unwrap();
        m.retain(|_, (at, _)| at.elapsed() < TTL);
        match m.get(key) {
            Some((_, State::Done(v))) => return Ok(Begin::Replay(v.clone())),
            Some((_, State::InFlight)) => return Err(Refusal::InFlight),
            Some((_, State::Unknown)) => return Err(Refusal::Unknown),
            None => {}
        }
        m.insert(key.to_string(), (Instant::now(), State::InFlight));
        Ok(Begin::Go(Permit {
            guard: self.clone(),
            key: key.to_string(),
            settled: false,
        }))
    }

    fn set(&self, key: &str, state: Option<State>) {
        let mut m = self.map.lock().unwrap();
        match state {
            Some(s) => m.insert(key.to_string(), (Instant::now(), s)),
            None => m.remove(key),
        };
    }
}

/// Held for the length of a send. Dropped without being settled (a cancelled or panicked
/// call) it leaves the key "unknown", which is the safe reading.
pub struct Permit {
    guard: Guard,
    key: String,
    settled: bool,
}

impl Permit {
    pub fn done(mut self, result: Value) {
        self.settled = true;
        self.guard.set(&self.key, Some(State::Done(result)));
    }
    /// Nothing was sent; the key is free again.
    pub fn release(mut self) {
        self.settled = true;
        self.guard.set(&self.key, None);
    }
    /// The message may have been sent.
    pub fn unknown(mut self) {
        self.settled = true;
        self.guard.set(&self.key, Some(State::Unknown));
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if !self.settled {
            self.guard.set(&self.key, Some(State::Unknown));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn go(g: &Guard, k: &str) -> Permit {
        match g.begin(k) {
            Ok(Begin::Go(p)) => p,
            _ => panic!("expected a reservation"),
        }
    }

    #[test]
    fn second_caller_is_refused_while_first_is_in_flight() {
        let g = Guard::default();
        let _p = go(&g, "send_draft:e1");
        assert_eq!(g.begin("send_draft:e1").err(), Some(Refusal::InFlight));
        assert!(matches!(g.begin("send_draft:e2"), Ok(Begin::Go(_))));
    }

    #[test]
    fn success_replays_and_definite_failure_releases() {
        let g = Guard::default();
        go(&g, "a").done(json!({"sent": true}));
        assert!(matches!(g.begin("a"), Ok(Begin::Replay(v)) if v["sent"] == true));
        go(&g, "b").release();
        assert!(matches!(g.begin("b"), Ok(Begin::Go(_))));
    }

    #[test]
    fn ambiguous_failure_and_dropped_permit_block_retries() {
        let g = Guard::default();
        go(&g, "a").unknown();
        assert_eq!(g.begin("a").err(), Some(Refusal::Unknown));
        drop(go(&g, "b"));
        assert_eq!(g.begin("b").err(), Some(Refusal::Unknown));
    }

    #[test]
    fn threads_race_for_one_key_and_exactly_one_wins() {
        let g = Guard::default();
        let wins: usize = (0..16)
            .map(|_| {
                let g = g.clone();
                std::thread::spawn(move || match g.begin("k") {
                    Ok(Begin::Go(p)) => {
                        std::mem::forget(p); // keep the reservation held
                        true
                    }
                    _ => false,
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap() as usize)
            .sum();
        assert_eq!(wins, 1);
    }
}
