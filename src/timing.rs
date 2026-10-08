//! `-t`: how long each phase took, on stderr, so stdout stays the answer.

use std::time::{Duration, Instant};

pub(crate) struct Timer {
    start: Instant,
    last: Instant,
    phases: Vec<(String, Duration)>,
}

impl Timer {
    pub(crate) fn new() -> Self {
        let now = Instant::now();
        Self { start: now, last: now, phases: Vec::new() }
    }
    pub(crate) fn lap(&mut self, name: impl Into<String>) {
        let now = Instant::now();
        self.phases.push((name.into(), now - self.last));
        self.last = now;
    }
    pub(crate) fn report(&self) {
        let parts: Vec<_> = self.phases.iter().map(|(n, d)| format!("{n} {}ms", d.as_millis())).collect();
        eprintln!("⏱  {} · total {}ms", parts.join(" · "), self.start.elapsed().as_millis());
    }
}
