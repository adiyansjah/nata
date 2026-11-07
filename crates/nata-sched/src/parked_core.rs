//! Small real-core models: initialize two already-claimed jobs so file opening,
//! output formatting and I/O-thread setup do not multiply the parking state space.
use super::*;
use crate::sync::{AtomicUsize, spawn_named};

struct Task {
    id: usize,
    live: Arc<AtomicUsize>,
}
impl Drop for Task {
    fn drop(&mut self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }
}
struct Jobs;
impl Work for Jobs {
    type Task = Task;
    type Open = ();
    type Out = usize;
    type Err = ();
    fn files(&self) -> usize {
        0
    }
    fn open(&self, _: usize, _: Option<()>, _: &Parker<'_>) -> OpenStep<Task, (), ()> {
        OpenStep::Ready(Ok(vec![]))
    }
    fn run(&self, task: &mut Task, _: &CancelToken<'_>) -> Run<usize, ()> {
        Run::Done(task.id)
    }
    fn internal_error(&self, _: &str) {}
}
fn model(f: impl Fn() + Send + Sync + 'static) {
    #[cfg(nata_loom)]
    {
        let mut b = loom::model::Builder::new();
        b.preemption_bound = Some(
            std::env::var("LOOM_MAX_PREEMPTIONS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(2),
        );
        b.check(f);
    }
    #[cfg(not(nata_loom))]
    f();
}

#[test]
fn two_workers_wake_and_cancel_two_parked_jobs() {
    for mode in 0..3 {
        model(move || {
            let scheduler = Scheduler::new(
                Arc::new(Jobs),
                Config {
                    threads: 2,
                    window: 2,
                    ordered: true,
                    open_ahead: 1,
                },
            );
            let sh = scheduler.sh.clone();
            {
                let mut g = lock(&sh.core);
                g.claimed = 2;
                g.high_water = 2;
                g.exhausted = true;
                g.running.insert(Key::Run(0), false);
                g.running.insert(Key::Run(1), false);
            }
            let live = Arc::new(AtomicUsize::new(2));
            let workers: Vec<_> = (0..2)
                .map(|id| {
                    let sh = sh.clone();
                    let live = live.clone();
                    spawn_named("park".into(), move || {
                        park_task(&sh, id as u64, Task { id, live })
                    })
                    .expect("CPU worker")
                })
                .collect();
            let io = if mode < 2 {
                let sh = sh.clone();
                Some(
                    spawn_named("wake".into(), move || {
                        let order = if mode == 0 { [0, 1] } else { [1, 0] };
                        for seq in order {
                            sh.wake_key(Key::Run(seq));
                        }
                    })
                    .expect("I/O worker"),
                )
            } else {
                None
            };
            // Both CPU workers have returned, even if I/O has not answered.
            for h in workers {
                assert!(sync::join(h));
            }
            if let Some(io) = io {
                for _ in 0..2 {
                    let Some(Job::Run(seq, task)) = claim(&sh) else {
                        panic!("woken job missing");
                    };
                    publish(&sh, seq, Run::Done(task.id));
                    drop(task);
                }
                assert!(sync::join(io));
                let g = lock(&sh.core);
                assert!(matches!(g.slots.get(&0), Some(Slot::Done(0))));
                assert!(matches!(g.slots.get(&1), Some(Slot::Done(1))));
                assert!(g.parked_tasks.is_empty() && g.resume.is_empty());
            } else {
                assert_eq!(
                    lock(&sh.core).parked_tasks.len(),
                    2,
                    "reachable cancel canary"
                );
                sh.cancel_all();
                assert_eq!(lock(&sh.core).parked_tasks.len(), 0);
                assert_eq!(lock(&sh.core).slots.len(), 2);
            }
            assert_eq!(lock(&sh.core).parks, 2);
            assert!(lock(&sh.core).high_water <= 2);
            assert_eq!(live.load(Ordering::SeqCst), 0);
        });
    }
}
