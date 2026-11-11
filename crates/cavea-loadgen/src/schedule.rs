//! Open-loop scheduling: work starts on a clock, not when earlier work ends,
//! so a slow server shows as latency and backlog rather than as a lower
//! offered rate. In-flight work is bounded; a start that finds the bound
//! reached is skipped and counted, never delayed.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Schedule {
    /// Starts the clock called for.
    pub offered: u64,
    pub started: u64,
    /// Starts skipped because `max_in_flight` were running.
    pub skipped: u64,
    /// From the first start to the last due start.
    pub span: Duration,
    /// From the first start until every started task ended.
    pub elapsed: Duration,
}

/// Start `f(k, due)` for `k` in `0..count` at `start + k / rate`, at most
/// `max_in_flight` at once. Latency should be measured from `due`, so that a
/// start the generator itself delayed still counts against the server.
pub async fn open_loop<F, Fut>(
    start: Instant,
    rate: f64,
    count: u64,
    max_in_flight: usize,
    mut f: F,
) -> Schedule
where
    F: FnMut(u64, Instant) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let permits = Arc::new(Semaphore::new(max_in_flight.max(1)));
    let mut tasks = JoinSet::new();
    let mut s = Schedule {
        offered: count,
        ..Schedule::default()
    };
    let rate = if rate > 0.0 { rate } else { f64::INFINITY };
    for k in 0..count {
        let due = start + Duration::from_secs_f64(k as f64 / rate);
        sleep_until(due).await;
        s.span = due - start;
        match permits.clone().try_acquire_owned() {
            Ok(permit) => {
                s.started += 1;
                let work = f(k, due);
                tasks.spawn(async move {
                    work.await;
                    drop(permit);
                });
            }
            Err(_) => s.skipped += 1,
        }
        while tasks.try_join_next().is_some() {}
    }
    while tasks.join_next().await.is_some() {}
    s.elapsed = start.elapsed();
    s
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A server that takes a second per request does not slow the starts:
    /// 200 starts at 100 a second all begin on time; with room for only 50 in
    /// flight, the others are skipped and counted, still on the same clock.
    #[tokio::test(start_paused = true)]
    async fn slow_responses_do_not_lower_the_offered_rate() {
        // Bounded at 50, each second starts 50 and skips 50.
        for (bound, started) in [(1_000, 200..=200), (50, 99..=101)] {
            let begin = Instant::now();
            let starts = Arc::new(Mutex::new(Vec::new()));
            let s = open_loop(begin, 100.0, 200, bound, |k, due| {
                let starts = starts.clone();
                async move {
                    starts.lock().unwrap().push((k, Instant::now() - due));
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            })
            .await;
            assert_eq!((s.offered, s.started + s.skipped), (200, 200));
            assert!(started.contains(&s.started), "{s:?}");
            assert_eq!(s.span, Duration::from_millis(1_990));
            // The last start, at 1.99 s, or at 1.49 s or 1.5 s, ends a second later.
            let last = s.elapsed.as_millis();
            assert!(
                if bound == 50 {
                    (2_490..=2_500).contains(&last)
                } else {
                    last == 2_990
                },
                "{s:?}"
            );
            let starts = starts.lock().unwrap();
            assert_eq!(starts.len() as u64, s.started);
            assert!(
                starts.iter().all(|(_, late)| late.is_zero()),
                "a start was delayed"
            );
        }
    }

    /// When the generator itself stalls, the work started late is still
    /// given its scheduled `due`, so latency measured from it includes the
    /// stall: a blocked thread does not hide a slow server.
    #[test]
    fn a_stalled_generator_is_charged_from_the_schedule() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let lags = Arc::new(Mutex::new(Vec::new()));
        let begin = Instant::now();
        let s = rt.block_on(open_loop(begin, 1_000.0, 20, 100, |k, due| {
            let lags = lags.clone();
            async move {
                assert_eq!(due, begin + Duration::from_millis(k));
                if k == 0 {
                    // The only thread is blocked for 30 ms.
                    std::thread::sleep(Duration::from_millis(30));
                }
                lags.lock().unwrap().push(Instant::now() - due);
            }
        }));
        assert_eq!(s.started, 20);
        let lags = lags.lock().unwrap();
        // Starts 1 to 19 were due in the stall and ran after it.
        assert!(
            lags.iter().skip(1).all(|l| *l >= Duration::from_millis(10)),
            "{lags:?}"
        );
    }
}
