//! Production ownership/parking seam. Only these primitives are replaced by loom;
//! sockets, Tokio timers and the scan implementation are covered by live tests.
#[cfg(nata_portal_loom)]
use loom::sync::{Condvar, Mutex};
#[cfg(not(nata_portal_loom))]
use std::sync::{Condvar, Mutex};

pub(super) trait Cancel: Clone {
    fn cancel(&self);
}

/// Selection and cancellation share a lock: suspension cannot publish before an
/// already selected cancellation finishes. A timer holds a cloned target instead.
pub(super) struct Slot<T>(Mutex<Option<T>>);
impl<T: Cancel> Slot<T> {
    pub(super) fn new(value: Option<T>) -> Self {
        Self(Mutex::new(value))
    }
    pub(super) fn install(&self, value: T) {
        *self.0.lock().expect("cancel slot") = Some(value);
    }
    pub(super) fn current(&self) -> Option<T> {
        self.0.lock().expect("cancel slot").clone()
    }
    pub(super) fn take(&self) -> Option<T> {
        self.0.lock().expect("cancel slot").take()
    }
    pub(super) fn cancel(&self) {
        if let Some(target) = self.0.lock().expect("cancel slot").as_ref() {
            target.cancel();
        }
    }
}

/// One owner wins expiry versus disconnect/Close cleanup.
pub(super) struct Once<T>(Mutex<Option<T>>);
impl<T> Once<T> {
    pub(super) fn new(value: T) -> Self {
        Self(Mutex::new(Some(value)))
    }
    pub(super) fn take(&self) -> Option<T> {
        self.0.lock().expect("cleanup owner").take()
    }
}

/// A single outstanding Execute credit, plus sticky close. Credits are published
/// under the same lock as wait, so resume/expiry before park cannot lose a wake.
pub(super) struct Credit {
    value: Mutex<(Option<usize>, bool)>,
    wake: Condvar,
}
impl Credit {
    pub(super) fn new() -> Self {
        Self {
            value: Mutex::new((None, false)),
            wake: Condvar::new(),
        }
    }
    pub(super) fn send(&self, rows: usize) -> Result<(), ()> {
        let mut value = self.value.lock().expect("credit");
        if value.1 {
            return Err(());
        }
        value.0 = Some(rows);
        self.wake.notify_all();
        Ok(())
    }
    pub(super) fn wait(&self) -> Result<usize, ()> {
        let mut value = self.value.lock().expect("credit");
        loop {
            if value.1 {
                return Err(());
            }
            if let Some(rows) = value.0.take() {
                return Ok(rows);
            }
            value = self.wake.wait(value).expect("credit wait");
        }
    }
    pub(super) fn close(&self) {
        self.value.lock().expect("credit").1 = true;
        self.wake.notify_all();
    }
}

/// Exact-end fetches suspend too. Zero means unlimited, including after resume.
pub(super) fn row(remaining: &mut usize) -> bool {
    if *remaining == 0 {
        return false;
    }
    *remaining -= 1;
    *remaining == 0
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Enter {
    Start,
    Complete,
    Expired,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Phase<T> {
    Bound,
    Running,
    Suspended(T),
    Complete,
    Expired,
}
impl<T: Copy> Phase<T> {
    pub(super) fn enter(&mut self) -> Enter {
        if matches!(self, Self::Complete | Self::Expired) {
            return if matches!(self, Self::Complete) {
                Enter::Complete
            } else {
                Enter::Expired
            };
        }
        *self = Self::Running;
        Enter::Start
    }
    pub(super) fn suspend(&mut self, at: T) {
        assert!(matches!(self, Self::Running));
        *self = Self::Suspended(at);
    }
    pub(super) fn complete(&mut self) {
        assert!(matches!(self, Self::Running));
        *self = Self::Complete;
    }
    pub(super) fn expire(&mut self) {
        assert!(matches!(self, Self::Suspended(_)));
        *self = Self::Expired;
    }
    pub(super) fn suspended(&self) -> Option<T> {
        match self {
            Self::Suspended(at) => Some(*at),
            _ => None,
        }
    }
}

/// Extended error drain and wall deadline ownership, independent of clock type.
/// The production event loop supplies Instant; models supply logical ticks.
pub(super) struct Exchange<T> {
    pub(super) failed: bool,
    deadline: Option<T>,
}
impl<T: Copy + Ord> Exchange<T> {
    pub(super) fn new() -> Self {
        Self {
            failed: false,
            deadline: None,
        }
    }
    pub(super) fn begin(&mut self, deadline: Option<T>) {
        if self.deadline.is_none() {
            self.deadline = deadline;
        }
    }
    pub(super) fn ignoring(&self) -> bool {
        self.failed
    }
    pub(super) fn deadline(&self) -> Option<T> {
        self.deadline
    }
    pub(super) fn timed_out(&self, now: T) -> bool {
        !self.failed && self.deadline.is_some_and(|at| now >= at)
    }
    pub(super) fn sync(&mut self, transaction: u8) -> bool {
        self.deadline = None;
        self.failed = false;
        transaction == b'I'
    }
}
