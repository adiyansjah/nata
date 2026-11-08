//! Small continuation adapter for query-owned spill jobs. A CPU worker executes
//! one step, then returns the task to the dispatcher. Blocked tasks register a
//! sticky wake and consume no CPU worker while the disk service runs.
use crate::memory::Wake;
use crate::sync::{Arc, Condvar, Mutex, scope, sync_channel};

pub enum Step<O, E> {
    Yield,
    Blocked,
    Done(Result<O, E>),
}
#[derive(Default)]
struct Signal {
    ready: Mutex<bool>,
    changed: Condvar,
}
impl Wake for Signal {
    fn wake(&self) {
        *self
            .ready
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        self.changed.notify_one();
    }
}
/// Run bounded continuation slots. The dispatcher owns parked task state; each
/// call to `step` is finite and must poll cancellation inside long CPU loops.
/// A wake only marks readiness; it never invokes the operator under a lock.
/// The native dispatcher is intentionally outside loom's memory-service models.
pub fn run<T: Send, O: Send, E: Send>(
    tasks: Vec<T>,
    threads: usize,
    step: impl Fn(&mut T, Arc<dyn Wake>) -> Step<O, E> + Sync,
    sink: impl FnMut(O) -> Result<(), E>,
) -> Result<(), E> {
    run_observed(None, tasks, threads, step, sink)
}

/// The same wave policy, with opt-in observations; no scheduling decisions use stats.
pub fn run_observed<T: Send, O: Send, E: Send>(
    stats: Option<&Arc<crate::query_stats::QueryStats>>,
    tasks: Vec<T>,
    threads: usize,
    step: impl Fn(&mut T, Arc<dyn Wake>) -> Step<O, E> + Sync,
    mut sink: impl FnMut(O) -> Result<(), E>,
) -> Result<(), E> {
    let stats = stats.filter(|_| crate::query_stats::QueryStats::available());
    let pipeline = stats.map_or(0, |s| s.pipeline());
    let wave = crate::sync::AtomicU64::new(0);
    struct Slot<T> {
        task: Option<T>,
        signal: Arc<Signal>,
    }
    let mut slots: Vec<_> = tasks
        .into_iter()
        .map(|task| {
            let signal = Arc::new(Signal::default());
            signal.wake();
            Slot {
                task: Some(task),
                signal,
            }
        })
        .collect();
    let threads = threads.max(1).min(slots.len().max(1));
    scope(|scope| {
        let (jobs, receive) = sync_channel::<(usize, T, Arc<Signal>)>(threads);
        let receive = Arc::new(Mutex::new(receive));
        let (completed, completions) = sync_channel(threads);
        for _ in 0..threads {
            let receive = Arc::clone(&receive);
            let completed = completed.clone();
            let step = &step;
            let wave = &wave;
            scope.spawn(move || {
                loop {
                    let job = receive
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .recv();
                    let Ok((i, mut task, signal)) = job else {
                        break;
                    };
                    let owner = stats.cloned();
                    let _scope = crate::query_stats::Scope::new(&owner);
                    let wave_id = if stats.is_some() {
                        wave.load(crate::sync::Ordering::Relaxed)
                    } else {
                        0
                    };
                    let cpu = stats.and_then(|_| crate::query_stats::thread_cpu_ns());
                    if let Some(s) = stats {
                        s.event("step_start", pipeline, i as u64, wave_id, 0);
                    }
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        step(&mut task, signal)
                    }));
                    if let Some(s) = stats {
                        let status = match &result {
                            Ok(Step::Yield) => "yield",
                            Ok(Step::Blocked) => "blocked",
                            _ => "done",
                        };
                        let cpu = cpu
                            .zip(crate::query_stats::thread_cpu_ns())
                            .map_or(u64::MAX, |(a, b)| b.saturating_sub(a));
                        s.event(status, pipeline, i as u64, wave_id, cpu);
                    }
                    if completed.send((i, task, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(completed);
        let mut result = Ok(());
        while slots.iter().any(|s| s.task.is_some()) && result.is_ok() {
            let ready: Vec<_> = slots
                .iter_mut()
                .enumerate()
                .filter(|(_, s)| s.task.is_some())
                .filter(|(_, s)| {
                    let mut ready = s
                        .signal
                        .ready
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    std::mem::take(&mut *ready)
                })
                .take(threads)
                .map(|(i, s)| {
                    (
                        i,
                        s.task.take().expect("active task"),
                        Arc::clone(&s.signal),
                    )
                })
                .collect();
            if ready.is_empty() {
                if let Some(s) = stats {
                    s.event("dispatcher_wait_start", pipeline, 0, 0, 0);
                }
                // This dispatcher wait holds no CPU slot. Periodic cancellation
                // polls also cover an external I/O service that never wakes.
                if let Some(s) = slots.iter().find(|s| s.task.is_some()) {
                    let ready = s
                        .signal
                        .ready
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    drop(
                        s.signal
                            .changed
                            .wait_timeout(ready, std::time::Duration::from_millis(10)),
                    );
                }
                for s in slots.iter().filter(|s| s.task.is_some()) {
                    s.signal.wake();
                }
                if let Some(s) = stats {
                    s.event("dispatcher_wait_end", pipeline, 0, 0, 0);
                }
                continue;
            }
            let wave_id = if stats.is_some() {
                wave.fetch_add(1, crate::sync::Ordering::Relaxed) + 1
            } else {
                0
            };
            let count = ready.len();
            if let Some(s) = stats {
                s.event("wave_start", pipeline, 0, wave_id, count as u64);
            }
            for job in ready {
                if let Some(s) = stats {
                    s.event("dispatch", pipeline, job.0 as u64, wave_id, 0);
                }
                jobs.send(job)
                    .unwrap_or_else(|_| panic!("continuation worker exited"));
            }
            let mut outputs = Vec::new();
            for _ in 0..count {
                let (i, task, status) = completions.recv().expect("continuation completion");
                let slot = &mut slots[i];
                match status.unwrap_or_else(|p| std::panic::resume_unwind(p)) {
                    Step::Yield => {
                        slot.task = Some(task);
                        slot.signal.wake();
                    }
                    Step::Blocked => {
                        slot.task = Some(task);
                    }
                    Step::Done(out) => {
                        drop(task);
                        outputs.push(out);
                    }
                }
            }
            if let Some(s) = stats {
                s.event("wave_end", pipeline, 0, wave_id, count as u64);
            }
            // Every step in this wave has returned before invoking the sink.
            // A sink may run another dependent phase without retaining workers.
            for out in outputs {
                if result.is_ok() {
                    result = out.and_then(&mut sink);
                }
            }
        }
        drop(jobs);
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn early_wake_and_parked_tasks_do_not_occupy_the_worker() {
        let order = Mutex::new(Vec::new());
        let mut phase = 0;
        run(
            vec![(0, 0), (1, 0)],
            1,
            |task, wake| {
                task.1 += 1;
                if task.0 == 0 && task.1 == 1 {
                    // Completion before returning Blocked must survive registration.
                    wake.wake();
                    return Step::Blocked;
                }
                order.lock().expect("order").push(task.0);
                Step::Done(Ok::<_, ()>(task.0))
            },
            |_| {
                phase += 1;
                Ok(())
            },
        )
        .expect("continuations");
        assert_eq!(phase, 2);
        assert_eq!(*order.lock().expect("order"), [0, 1]);
    }
    #[test]
    fn sink_failure_joins_running_steps_and_drops_parked_state() {
        struct Owner(Arc<crate::sync::AtomicUsize>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.0.fetch_sub(1, crate::sync::Ordering::SeqCst);
            }
        }
        let live = Arc::new(crate::sync::AtomicUsize::new(4));
        let tasks = (0..4).map(|_| Owner(live.clone())).collect();
        assert_eq!(
            run(
                tasks,
                2,
                |_, _| Step::Done(Ok::<_, &str>(())),
                |_| Err("sink")
            ),
            Err("sink")
        );
        assert_eq!(live.load(crate::sync::Ordering::SeqCst), 0);
    }
}
