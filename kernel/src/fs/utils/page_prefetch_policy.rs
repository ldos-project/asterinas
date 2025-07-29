use alloc::{borrow::ToOwned, boxed::Box, sync::Arc, vec::Vec};
use core::{
    error::Error,
    mem::{forget, ManuallyDrop},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use log::info;
use ostd::{
    error_result, path,
    sync::{Mutex, WaitQueue, Waiter},
    tables::{
        locking::ObservableLockingTable, registry::get_global_table_registry,
        spsc::SpscTableCustom, Producer, Table, WeakObserver,
    },
};

use crate::{
    sched::{RealTimePolicy, SchedPolicy},
    thread::{kernel_thread::ThreadOptions, Tid},
    time::{clocks::MonotonicClock, timer::Timeout},
};

const REGISTRATION_TABLE_BUFFER_SIZE: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct PrefetchCommand {
    pub page: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct PageAccessEvent {
    pub timestamp: Duration,
    pub thread: Option<Tid>,
    pub page: usize,
    pub access_type: AccessType,
}

#[derive(Clone, Copy, Debug)]
pub enum AccessType {
    Read,
    Write,
}

#[derive(Clone)]
pub struct PageCacheRegistrationCommand {
    /// The table to send prefetch commands to.
    pub prefetch_command_table: Arc<dyn Table<PrefetchCommand>>,
    /// The table to observe to see the pages accessed via this page cache.
    pub access_table: Arc<dyn Table<PageAccessEvent>>,
}

pub struct PageCacheRegistration {
    /// The table to send prefetch commands to.
    pub prefetch_command_producer: Box<dyn Producer<PrefetchCommand>>,
    /// The table to observe to see the pages accessed via this page cache.
    pub access_observer: Box<dyn WeakObserver<PageAccessEvent>>,
}

static_assertions::assert_impl_all!(PageCacheRegistration: Send);

pub fn start_prefetch_policy() -> Result<(), Box<dyn Error>> {
    static ALREADY_STARTED: AtomicBool = AtomicBool::new(false);
    if !ALREADY_STARTED.swap(true, Ordering::Relaxed) {
        info!("Starting prefetch policy");
        let prefetcher_registration_table =
            ObservableLockingTable::<PageCacheRegistrationCommand>::new(
                REGISTRATION_TABLE_BUFFER_SIZE,
                1,
            );
        let registry = get_global_table_registry();
        registry.register(
            path!(pagecache.policy.registration),
            prefetcher_registration_table.clone(),
        );

        let prefetcher_registration_consumer = prefetcher_registration_table.attach_consumer()?;

        // WE HAVE TO LEAK THIS. SEE TODO IN ObserverableLockingTable
        forget(prefetcher_registration_table);

        let timer_table = SpscTableCustom::<_, WaitQueue>::new(2, 0, 0);
        let timer_table_producer = Mutex::new(timer_table.attach_producer()?);
        let timer_table_consumer = timer_table.attach_consumer()?;

        let prefetch_timer =
            ManuallyDrop::new(MonotonicClock::timer_manager().create_timer(move || {
                timer_table_producer.lock().try_put(());
            }));
        prefetch_timer.set_interval(Duration::from_micros(1));
        prefetch_timer.set_timeout(Timeout::After(Duration::from_micros(10)));

        info!("Prefetch policy starting");
        ThreadOptions::new(move || {
            info!("Prefetch policy started");
            let mut registrations = Vec::new();
            let (registration_waiter, _) = Waiter::new_pair();
            // Start assuming this is ready (to get it registered properly)
            registration_waiter.waker().wake_up();
            let (timer_waiter, _) = Waiter::new_pair();
            timer_waiter.waker().wake_up();

            let mut last_prefetch = (0, 0);

            loop {
                let wake_index =
                    Waiter::wait_many_iter([&registration_waiter, &timer_waiter].into_iter());
                match wake_index {
                    0 => {
                        let mut register = || {
                            prefetcher_registration_consumer
                                .enqueue_for_take(registration_waiter.waker());
                            if let Some(reg) = prefetcher_registration_consumer.try_take() {
                                info!("registered new page cache");
                                registrations.push(PageCacheRegistration {
                                    prefetch_command_producer: reg
                                        .prefetch_command_table
                                        .attach_producer()?,
                                    access_observer: reg.access_table.attach_weak_observer()?,
                                });
                            }
                            Ok::<_, Box<dyn Error>>(())
                        };
                        error_result!(register())
                    }
                    1 => {
                        timer_table_consumer.enqueue_for_take(timer_waiter.waker());
                        if let Some(()) = timer_table_consumer.try_take() {
                            for (
                                reg_i,
                                PageCacheRegistration {
                                    prefetch_command_producer,
                                    access_observer,
                                },
                            ) in registrations.iter().enumerate()
                            {
                                // POLICY
                                let recent = access_observer.recent_cursor();
                                let observations = (recent - 10..recent)
                                    .flat_map(|i| access_observer.weak_observe(i))
                                    .collect::<Vec<_>>();
                                if observations.len() > 5
                                    && let Some(max) = observations
                                        .iter()
                                        .map(|PageAccessEvent { page, .. }| page)
                                        .max()
                                    && last_prefetch != (reg_i, *max)
                                {
                                    let put_res = prefetch_command_producer
                                        .try_put(PrefetchCommand { page: max + 10 });
                                    info!(
                                        "Prefetching {max} for {reg_i}: send? {}",
                                        put_res.is_none()
                                    );
                                    last_prefetch = (reg_i, *max);
                                }
                            }
                        }
                    }
                    _ => unreachable!(),
                }
            }
        })
        .sched_policy(SchedPolicy::RealTime {
            rt_prio: 1.try_into().unwrap(),
            rt_policy: RealTimePolicy::default(),
        })
        .spawn();

        start_prefetch_data_dumper()?;

        Ok(())
    } else {
        Ok(())
    }
}

/// Start a server which collects the page caches and periodically prints their history.
fn start_prefetch_data_dumper() -> Result<(), Box<dyn Error>> {
    let registry = get_global_table_registry();
    let prefetcher_registration_table = registry
        .lookup::<PageCacheRegistrationCommand>(&path!(pagecache.policy.registration))
        .ok_or_else(|| ostd::tables::TableAttachError::Whatever {
            message: "missing table".to_owned(),
            source: None,
        })?;

    let prefetcher_registration_observer =
        prefetcher_registration_table.attach_strong_observer()?;

    let timer_table = SpscTableCustom::<_, WaitQueue>::new(2, 0, 0);
    let timer_table_producer = Mutex::new(timer_table.attach_producer()?);
    let timer_table_consumer = timer_table.attach_consumer()?;

    let prefetch_timer =
        ManuallyDrop::new(MonotonicClock::timer_manager().create_timer(move || {
            timer_table_producer.lock().try_put(());
        }));
    prefetch_timer.set_interval(Duration::from_millis(500));
    prefetch_timer.set_timeout(Timeout::After(Duration::from_millis(1000)));

    ThreadOptions::new(move || {
        let mut registrations = Vec::new();
        let (registration_waiter, _) = Waiter::new_pair();
        // Start assuming this is ready (to get it registered properly)
        registration_waiter.waker().wake_up();
        let (timer_waiter, _) = Waiter::new_pair();
        timer_waiter.waker().wake_up();

        loop {
            let wake_index =
                Waiter::wait_many_iter([&registration_waiter, &timer_waiter].into_iter());
            match wake_index {
                0 => {
                    let mut register = || {
                        prefetcher_registration_observer
                            .enqueue_for_strong_observe(registration_waiter.waker());
                        if let Some(reg) = prefetcher_registration_observer.try_strong_observe() {
                            info!("start_prefetch_data_dumper: registered new page cache");
                            registrations.push((
                                reg.prefetch_command_table.attach_weak_observer()?,
                                reg.access_table.attach_weak_observer()?,
                            ));
                        }
                        Ok::<_, Box<dyn Error>>(())
                    };
                    error_result!(register())
                }
                1 => {
                    timer_table_consumer.enqueue_for_take(timer_waiter.waker());
                    if let Some(()) = timer_table_consumer.try_take() {
                        for (prefetch_command_observer, access_observer) in registrations.iter() {
                            info!(
                                "prefetch commands: {:?}",
                                prefetch_command_observer.weak_observer_range(
                                    prefetch_command_observer.oldest_cursor(),
                                    prefetch_command_observer.recent_cursor()
                                )
                            );
                            info!(
                                "accesses: {:?}",
                                access_observer.weak_observer_range(
                                    access_observer.oldest_cursor(),
                                    access_observer.recent_cursor()
                                )
                            );
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
    })
    .sched_policy(SchedPolicy::RealTime {
        rt_prio: 1.try_into().unwrap(),
        rt_policy: RealTimePolicy::default(),
    })
    .spawn();

    Ok(())
}
