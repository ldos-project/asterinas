use alloc::{borrow::ToOwned as _, boxed::Box, vec::Vec};
use core::time::Duration;

use hashbrown::HashMap;
use log::{error, info};
use ostd::{
    error_result, path,
    prelude::println,
    sync::Waiter,
    tables::{
        registry::get_global_table_registry, Consumer, Producer,
        StrongObserver, Table, WeakObserver,
    },
};

use super::{PageCacheRegistrationCommand, PrefetchCommand};
use crate::{
    fs::utils::page_prefetch_policy::new_waiter,
    sched::{RealTimePolicy, SchedPolicy},
    thread::kernel_thread::ThreadOptions,
    time::{
        clocks::BootTimeClock,
        Clock,
    },
};

pub fn start_reactive_prefetch_policy() -> core::result::Result<(), Box<dyn core::error::Error>> {
    let registry = get_global_table_registry();
    let prefetcher_registration_table = registry
        .lookup::<PageCacheRegistrationCommand>(&path!(pagecache.policy.registration))
        .ok_or_else(|| ostd::tables::TableAttachError::Whatever {
            message: "missing table".to_owned(),
            source: None,
        })?;

    let prefetcher_registration_consumer = prefetcher_registration_table.attach_consumer()?;

    ThreadOptions::new(move || reactive_policy_thread_fn(prefetcher_registration_consumer))
        .sched_policy(SchedPolicy::RealTime {
            rt_prio: 1.try_into().unwrap(),
            rt_policy: RealTimePolicy::default(),
        })
        .spawn();
    Ok(())
}

pub struct PageCacheRegistration {
    /// The table to send prefetch commands to.
    pub prefetch_command_producer: Box<dyn Producer<PrefetchCommand>>,
    /// The table to observe to see the pages accessed via this page cache.
    pub access_observer: Box<dyn StrongObserver<super::PageAccessEvent>>,
}

/// The body of the prefetch policy thread. This is spawned in [`start_prefetch_policy`].
fn reactive_policy_thread_fn(
    prefetcher_registration_consumer: Box<dyn Consumer<PageCacheRegistrationCommand>>,
) {
    info!("Reactive prefetch policy started");

    // The maximum time between a read on a thread and a prefetch.
    let maximum_prefetch_delay = Duration::from_micros(100000);
    // number of events to look at. If this is higher than the number of event available, this will operate on the data
    // that is available.
    let history_len = 16;
    // The minimum number of observations to see for a given thread before trying to prefetch.
    let min_observation_for_prefetch = 8;
    // The number of strides ahead of the most recent access to prefetch. This is needed to make sure the prefetch can
    // usefully complete before the reader actually reaches it.
    let strides_ahead_to_prefetch = &(4..32);

    // The terrible mess of waiters and tables and that horrific `match` works as follows:
    //
    // 1. Setup a waiter for each attachment and trigger an initial wake so that we get registered with the table
    //    correctly. (We could also do an explicit initial enqueue on the attachment, but this seemed less duplicitive.)
    // 2. Use `wait_many_iter` to wait on all the waiters at the same time. This returns an index into the iterator it
    //    was passed.
    // 3. Match on the index to select the code to execute based on what actually waked.
    // 4. For whatever attachment waked us, enqueue our waker again (done first to avoid a "missed wake" race) and then
    //    `try_take` the message.
    // 5. Process the message appropriately and then start from step 2.
    //
    // This is abjectly terrible and needs to be abstracted away with some form of event handling construct similar to
    // `select!` from tokio, or maybe a more abstract event driven programming thing that encapsulates the loop as well.
    //
    // NOTE: There may be a case here where our waker ends up registered more than once. It shouldn't hurt anything, but
    // might be wasteful and should definitely avoided in an abstraction for general use.

    let mut access_observers = None;
    let mut prefetch_command_producer = None;
    // let mut access_weak_observer = None;
    let access_waiter = new_waiter();

    let registration_waiter = new_waiter();

    // The last prefetched page for each thread. This is used to avoid performing the same prefetch repeatedly.
    let mut last_prefetch_for_thread = HashMap::new();

    loop {
        let wake_index = Waiter::wait_many_iter([&registration_waiter, &access_waiter].into_iter());
        match wake_index {
            0 => {
                let mut register = || {
                    prefetcher_registration_consumer.enqueue_for_take(registration_waiter.waker());
                    if let Some(reg) = prefetcher_registration_consumer.try_take() {
                        println!("reactive_policy_thread_fn: registered new page cache (discarding the old one)");
                        access_waiter.waker().wake_up();
                        access_observers = Some((
                            reg.access_table.attach_strong_observer()?,
                            reg.access_table.attach_weak_observer()?,
                        ));
                        prefetch_command_producer =
                            Some(reg.prefetch_command_table.attach_producer()?);
                    }
                    Ok::<_, Box<dyn core::error::Error>>(())
                };
                error_result!(register())
            }
            1 => {
                if let Some((strong_observer, weak_observer)) = &access_observers {
                    strong_observer.enqueue_for_strong_observe(access_waiter.waker());
                    if let Some(event) = strong_observer.try_strong_observe() {
                        // POLICY

                        // TODO: This allocates a bunch (Vecs and HashMaps). That should be removed.

                        let recent = weak_observer.recent_cursor();
                        let observations = (recent - history_len..recent)
                            .flat_map(|i| weak_observer.weak_observe(i))
                            .collect::<Vec<_>>();
                        let oldest_for_prefetch =
                            BootTimeClock::get().read_time() - maximum_prefetch_delay;

                        if observations.is_empty() {
                            continue;
                        }

                        // Spiritually: observations.filter(|o| o.thread.is_some()).group_by(|o| o.thread.unwrap())
                        // However, itertools doesn't support what that requires without std.
                        let mut observations_by_tid: HashMap<u32, Vec<_>> = HashMap::new();
                        for observation in observations.iter() {
                            if let Some(tid) = observation.thread {
                                let entry = observations_by_tid.entry(tid);
                                entry.or_default().push(observation);
                            }
                        }

                        // if observations_by_tid.len() > 1 {
                        //     info!("Threads {:?}", observations_by_tid.iter().map(|(k, v)| (k, v.len())).collect::<Vec<_>>())
                        // }

                        for (tid, observations) in observations_by_tid {
                            if observations.len() < min_observation_for_prefetch {
                                // this thread doesn't have enough observations.
                                continue;
                            }

                            let Some(last_event) = observations.last() else {
                                error!("There are no events for this thread: {}. This should be unreachable.", tid);
                                continue;
                            };

                            // if last_event.timestamp < oldest_for_prefetch {
                            //     // last observation is too old.
                            //     continue;
                            // }

                            fn counts<T: Eq + core::hash::Hash>(
                                this: impl Iterator<Item = T>,
                            ) -> HashMap<T, usize> {
                                let mut counts = HashMap::new();
                                this.for_each(|item| *counts.entry(item).or_default() += 1);
                                counts
                            }

                            let counted_strides = counts(observations.windows(2).map(|w| {
                                let [a, b] = w else {
                                    unreachable!("Incorrect window length.");
                                };
                                b.page - a.page
                            }));

                            let Some((most_common_stride, _)) =
                                counted_strides.iter().max_by_key(|e| e.1)
                            else {
                                // This thread has very view observations. This will be very rare.
                                continue;
                            };

                            let last_prefetch_page =
                                last_prefetch_for_thread.entry(tid).or_default();

                            for strides_ahead_to_prefetch in strides_ahead_to_prefetch.clone() {
                                let prefetch_page = last_event.page
                                    + most_common_stride * strides_ahead_to_prefetch;

                                if prefetch_page <= *last_prefetch_page {
                                    // Don't prefetch the same page twice in a row.
                                    continue;
                                }

                                info!("Attempting prefetch for thread {tid} with {} events: {last_event:?} {oldest_for_prefetch:?} {counted_strides:?} {most_common_stride} {last_prefetch_page} {prefetch_page}", observations.len());

                                // Update our state and send the prefetch
                                *last_prefetch_page = prefetch_page;
                                if let Some(prefetch_command_producer) = &prefetch_command_producer {
                                    let put_res =
                                        prefetch_command_producer.try_put(PrefetchCommand {
                                            page: prefetch_page,
                                        });
                                    if put_res.is_some() {
                                        info!("Prefetching {prefetch_page} for thread {tid} failed to send");
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => unreachable!(),
        }
    }
}
