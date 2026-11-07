//! Independent partition tasks. Workers steal from a deque; a bounded channel
//! publishes outputs without collecting a final table on the coordinator.
use crate::sync::{self, AtomicBool, Ordering};
use crossbeam_deque::{Injector, Steal};

/// Execute disjoint tasks, joining every worker on success, error or unwind.
/// `run` must check its cancellation predicate inside long tasks.
pub fn run<T: Send, O: Send, E: Send>(
    tasks: Vec<T>,
    threads: usize,
    cancelled: impl Fn() -> bool + Sync,
    run: impl Fn(T, &dyn Fn() -> bool) -> Result<O, E> + Sync,
    mut sink: impl FnMut(O) -> Result<(), E>,
) -> Result<(), E> {
    let queue = Injector::new();
    for task in tasks {
        queue.push(task);
    }
    let stop = AtomicBool::new(false);
    sync::scope(|scope| {
        let (tx, rx) = sync::sync_channel(threads.max(1));
        for _ in 0..threads.max(1) {
            let (queue, stop, run, cancelled, tx) = (&queue, &stop, &run, &cancelled, tx.clone());
            scope.spawn(move || {
                let is_cancelled = || stop.load(Ordering::SeqCst) || cancelled();
                while !is_cancelled() {
                    let task = match queue.steal() {
                        Steal::Success(t) => t,
                        Steal::Retry => continue,
                        Steal::Empty => break,
                    };
                    let result = run(task, &is_cancelled);
                    if tx.send(result).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx);
        let mut result = Ok(());
        // Drain after error so senders never block while the scope joins.
        for item in rx {
            if result.is_ok() {
                result = item.and_then(&mut sink);
            }
            if result.is_err() {
                stop.store(true, Ordering::SeqCst);
            }
        }
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::AtomicUsize;
    #[test]
    fn partitions_run_once_and_overlap() {
        let active = AtomicUsize::new(0);
        let peak = AtomicUsize::new(0);
        let mut seen = Vec::new();
        run(
            (0..32).collect(),
            4,
            || false,
            |i, _| {
                let n = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(n, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(5));
                active.fetch_sub(1, Ordering::SeqCst);
                Ok::<_, ()>(i)
            },
            |i| {
                seen.push(i);
                Ok(())
            },
        )
        .expect("tasks");
        seen.sort();
        assert_eq!(seen, (0..32).collect::<Vec<_>>());
        assert!(peak.load(Ordering::SeqCst) > 1);
    }
    #[test]
    fn sink_error_drains_and_joins() {
        let live = AtomicUsize::new(0);
        assert_eq!(
            run(
                (0..100).collect(),
                4,
                || false,
                |i, _| {
                    live.fetch_add(1, Ordering::SeqCst);
                    live.fetch_sub(1, Ordering::SeqCst);
                    Ok::<_, &str>(i)
                },
                |_| Err("sink")
            ),
            Err("sink")
        );
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }
}
