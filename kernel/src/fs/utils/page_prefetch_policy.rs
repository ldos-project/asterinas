use alloc::{borrow::ToOwned, boxed::Box, format, string::String, sync::Arc, vec::Vec};
use core::{
    mem::{forget, ManuallyDrop},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use aster_block::{BlockDevice, SECTOR_SIZE};
use aster_util::csv::ToCsv;
use core2::io::Write;
use hashbrown::HashMap;
use log::{error, info};
use ostd::{
    error_result, path,
    prelude::println,
    sync::{Mutex, WaitQueue, Waiter},
    tables::{
        locking::ObservableLockingTable, registry::get_global_table_registry, spsc::SpscTableCustom, Consumer, Producer, StrongObserver, Table, WeakObserver
    },
};

use crate::{
    fs::start_block_device,
    sched::{RealTimePolicy, SchedPolicy, SchedulingEvent},
    thread::{kernel_thread::ThreadOptions, Tid},
    time::{
        clocks::{BootTimeClock, MonotonicClock},
        timer::Timeout,
        Clock,
    },
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
    pub is_cache_hit: bool,
}

impl ToCsv for PageAccessEvent {
    fn to_csv(&self) -> String {
        format!(
            "{},{},{},{:?},{}",
            self.timestamp.as_secs_f32(),
            self.thread.unwrap_or_default(),
            self.page,
            self.access_type,
            self.is_cache_hit
        )
    }
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

pub fn start_prefetch_policy_subsystem() -> Result<(), Box<dyn core::error::Error>> {
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
        forget(prefetcher_registration_table);

        // start_prefetch_policy()?;

        // start_prefetch_data_dumper()?;
        // start_prefetch_data_logger()?;
        Ok(())
    } else {
        Ok(())
    }
}

pub fn start_prefetch_policy() -> Result<(), Box<dyn core::error::Error>> {
    // The interval between prefetch policy runs. This should be short enough to allow the prefetcher to act fast enough
    // to perform timely prefetches, but not so short that the overhead is high. A future implementation could be
    // triggered or the time could be modulated based I/O events.
    let prefetch_interval = Duration::from_micros(1);

    let registry = get_global_table_registry();
    let prefetcher_registration_table = registry
        .lookup::<PageCacheRegistrationCommand>(&path!(pagecache.policy.registration))
        .ok_or_else(|| ostd::tables::TableAttachError::Whatever {
            message: "missing table".to_owned(),
            source: None,
        })?;

    let prefetcher_registration_consumer = prefetcher_registration_table.attach_consumer()?;
    let timer_table = SpscTableCustom::<_, WaitQueue>::new(2, 0, 0);
    let timer_table_producer = Mutex::new(timer_table.attach_producer()?);
    let timer_table_consumer = timer_table.attach_consumer()?;
    let prefetch_timer =
        ManuallyDrop::new(MonotonicClock::timer_manager().create_timer(move || {
            timer_table_producer.lock().try_put(());
        }));
    prefetch_timer.set_interval(prefetch_interval);
    prefetch_timer.set_timeout(Timeout::After(prefetch_interval));

    ThreadOptions::new(move || {
        policy_thread_fn(prefetcher_registration_consumer, timer_table_consumer)
    })
    .sched_policy(SchedPolicy::RealTime {
        rt_prio: 1.try_into().unwrap(),
        rt_policy: RealTimePolicy::default(),
    })
    .spawn();
    Ok(())
}

/// The body of the prefetch policy thread. This is spawned in [`start_prefetch_policy`].
fn policy_thread_fn(
    prefetcher_registration_consumer: Box<dyn Consumer<PageCacheRegistrationCommand>>,
    timer_table_consumer: Box<dyn Consumer<()>>,
) {
    info!("Prefetch policy started");

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

    let mut registrations = Vec::new();
    let (registration_waiter, _) = Waiter::new_pair();
    registration_waiter.waker().wake_up();
    let (timer_waiter, _) = Waiter::new_pair();
    timer_waiter.waker().wake_up();

    // The last prefetched page for each thread. This is used to avoid performing the same prefetch repeatedly.
    let mut last_prefetch_for_thread = HashMap::new();

    loop {
        let wake_index = Waiter::wait_many_iter([&registration_waiter, &timer_waiter].into_iter());
        match wake_index {
            0 => {
                // Consume the page cache registrations and put them into a vec so the policy can handle each.
                let mut register = || {
                    prefetcher_registration_consumer.enqueue_for_take(registration_waiter.waker());
                    if let Some(reg) = prefetcher_registration_consumer.try_take() {
                        info!("registered new page cache");
                        registrations.push(PageCacheRegistration {
                            prefetch_command_producer: reg
                                .prefetch_command_table
                                .attach_producer()?,
                            access_observer: reg.access_table.attach_weak_observer()?,
                        });
                    }
                    Ok::<_, Box<dyn core::error::Error>>(())
                };
                error_result!(register())
            }
            1 => {
                // Then the timer elapses execute the policy for every registered page cache.
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

                        // TODO: This allocates a bunch (Vecs and HashMaps). That should be removed.

                        let recent = access_observer.recent_cursor();
                        let observations = (recent - history_len..recent)
                            .flat_map(|i| access_observer.weak_observe(i))
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

                                info!("Attempting prefetch for thread {tid} (cache {reg_i}) with {} events: {last_event:?} {oldest_for_prefetch:?} {counted_strides:?} {most_common_stride} {last_prefetch_page} {prefetch_page}", observations.len());

                                // Update our state and send the prefetch
                                *last_prefetch_page = prefetch_page;
                                let put_res = prefetch_command_producer.try_put(PrefetchCommand {
                                    page: prefetch_page,
                                });
                                if put_res.is_some() {
                                    info!("Prefetching {prefetch_page} for thread {tid} and page cache {reg_i} failed to send");
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

/// Start a server which collects the page caches and periodically prints their history.
fn start_prefetch_data_dumper() -> Result<(), Box<dyn core::error::Error>> {
    let registry = get_global_table_registry();
    let prefetcher_registration_table = registry
        .lookup::<PageCacheRegistrationCommand>(&path!(pagecache.policy.registration))
        .ok_or_else(|| ostd::tables::TableAttachError::Whatever {
            message: "missing table".to_owned(),
            source: None,
        })?;

    let scheduling_event_table = registry
        .lookup::<SchedulingEvent>(&path!(cpu.scheduler.events))
        .ok_or_else(|| ostd::tables::TableAttachError::Whatever {
            message: "missing table".to_owned(),
            source: None,
        })?;

    let scheduling_event_observer = scheduling_event_table.attach_weak_observer()?;

    let prefetcher_registration_observer =
        prefetcher_registration_table.attach_strong_observer()?;

    let timer_table = SpscTableCustom::<_, WaitQueue>::new(2, 0, 0);
    let timer_table_producer = Mutex::new(timer_table.attach_producer()?);
    let timer_table_consumer = timer_table.attach_consumer()?;

    let prefetch_timer =
        ManuallyDrop::new(MonotonicClock::timer_manager().create_timer(move || {
            timer_table_producer.lock().try_put(());
        }));
    prefetch_timer.set_interval(Duration::from_millis(2000));
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
                        Ok::<_, Box<dyn core::error::Error>>(())
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
                        info!(
                            "scheduling events: {:?}",
                            scheduling_event_observer.weak_observer_range(
                                scheduling_event_observer.oldest_cursor(),
                                scheduling_event_observer.recent_cursor()
                            )
                        );
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

/// Start a server which collects the page caches and periodically prints their history.
fn start_prefetch_data_logger() -> Result<(), Box<dyn core::error::Error>> {
    let registry = get_global_table_registry();
    let prefetcher_registration_table = registry
        .lookup::<PageCacheRegistrationCommand>(&path!(pagecache.policy.registration))
        .ok_or_else(|| ostd::tables::TableAttachError::Whatever {
            message: "missing table".to_owned(),
            source: None,
        })?;

    let scheduling_event_table = registry
        .lookup::<SchedulingEvent>(&path!(cpu.scheduler.events))
        .ok_or_else(|| ostd::tables::TableAttachError::Whatever {
            message: "missing table".to_owned(),
            source: None,
        })?;

    let scheduling_event_observer = scheduling_event_table.attach_weak_observer()?;

    let prefetcher_registration_observer =
        prefetcher_registration_table.attach_strong_observer()?;

    let log_block_device = start_block_device("vlog")?;

    let mut access_log = BufferedBlockDeviceWriter::new(log_block_device.clone(), 0);
    error_result!(access_log.write("timestamp,thread,page,type,is_cache_hit\n".as_bytes()));
    let mut scheduler_log = BufferedBlockDeviceWriter::new(log_block_device, 1024 * 1024 * 1024);
    error_result!(
        scheduler_log.write("timestamp,current_tid,current_thread_cpu_time,next_tid\n".as_bytes())
    );

    let scheduler_event_timer_table = SpscTableCustom::<_, WaitQueue>::new(2, 0, 0);
    let scheduler_event_timer_table_producer =
        Mutex::new(scheduler_event_timer_table.attach_producer()?);
    let scheduler_event_timer_table_consumer = scheduler_event_timer_table.attach_consumer()?;

    let scheduler_event_timer =
        ManuallyDrop::new(MonotonicClock::timer_manager().create_timer(move || {
            scheduler_event_timer_table_producer.lock().try_put(());
        }));
    scheduler_event_timer.set_interval(Duration::from_micros(2000));
    scheduler_event_timer.set_timeout(Timeout::After(Duration::from_millis(1)));

    let mut last_flush = MonotonicClock::get().read_time();

    ThreadOptions::new(move || {
        let mut access_observer = None;
        let registration_waiter = new_waiter();
        let scheduler_event_timer_waiter = new_waiter();
        let access_waiter = new_waiter();
        let mut last_scheduler_event = scheduling_event_observer.recent_cursor();

        loop {
            let wake_index = Waiter::wait_many_iter(
                [
                    &registration_waiter,
                    &access_waiter,
                    &scheduler_event_timer_waiter,
                ]
                .into_iter(),
            );
            match wake_index {
                0 => {
                    let mut register = || {
                        prefetcher_registration_observer
                            .enqueue_for_strong_observe(registration_waiter.waker());
                        if let Some(reg) = prefetcher_registration_observer.try_strong_observe() {
                            info!("start_prefetch_data_logger: registered new page cache (discarding the old one)");
                            access_waiter.waker().wake_up();
                            access_observer = Some(reg.access_table.attach_strong_observer()?);
                        }
                        Ok::<_, Box<dyn core::error::Error>>(())
                    };
                    error_result!(register())
                }
                1 => {
                    if let Some(access_observer) = &access_observer {
                        access_observer.enqueue_for_strong_observe(access_waiter.waker());
                        if let Some(event) = access_observer.try_strong_observe() {
                            let mut buf = Vec::new();
                            error_result!(buf.write(event.to_csv().as_bytes()));
                            buf.push(b'\n');
                            error_result!(access_log.write(&buf));
                        }
                    }
                }
                2 => {
                    scheduler_event_timer_table_consumer
                        .enqueue_for_take(scheduler_event_timer_waiter.waker());
                    if let Some(()) = scheduler_event_timer_table_consumer.try_take() {
                        let next = scheduling_event_observer.recent_cursor();
                        let mut buf = Vec::new();
                        let events = scheduling_event_observer
                            .weak_observer_range(last_scheduler_event, next);
                        // debug!("collected {} scheduler events.", events.len());
                        for event in events
                        {
                            buf.extend_from_slice(event.to_csv().as_bytes());
                            buf.push(b'\n');
                        }
                        error_result!(scheduler_log.write(&buf));
                        last_scheduler_event = next;

                        let now = MonotonicClock::get().read_time();

                        if now - last_flush > Duration::from_secs(5) {
                            info!("Flushing collected data.");
                            error_result!(scheduler_log.flush());
                            error_result!(access_log.flush());
                            last_flush = now;
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

/// Start a server that monitors the page hit rate and pariodically prints it.
pub fn start_page_hit_rate_monitor() -> Result<(), Box<dyn core::error::Error>> {
    static ALREADY_STARTED: AtomicBool = AtomicBool::new(false);
    if !ALREADY_STARTED.swap(true, Ordering::Relaxed) {
        let registry = get_global_table_registry();
        let prefetcher_registration_table = registry
            .lookup::<PageCacheRegistrationCommand>(&path!(pagecache.policy.registration))
            .ok_or_else(|| ostd::tables::TableAttachError::Whatever {
                message: "missing table".to_owned(),
                source: None,
            })?;

        let prefetcher_registration_observer =
            prefetcher_registration_table.attach_strong_observer()?;

        let print_timer_table = SpscTableCustom::<_, WaitQueue>::new(2, 0, 0);
        let print_timer_table_producer = Mutex::new(print_timer_table.attach_producer()?);
        let print_timer_table_consumer = print_timer_table.attach_consumer()?;

        let print_timer =
            ManuallyDrop::new(MonotonicClock::timer_manager().create_timer(move || {
                print_timer_table_producer.lock().try_put(());
            }));
        print_timer.set_interval(Duration::from_millis(2000));
        print_timer.set_timeout(Timeout::After(Duration::from_millis(1000)));

        ThreadOptions::new(move || {
            page_hit_rate_monitor_thread_fn(
                prefetcher_registration_observer,
                print_timer_table_consumer,
            )
        })
        .sched_policy(SchedPolicy::RealTime {
            rt_prio: 1.try_into().unwrap(),
            rt_policy: RealTimePolicy::default(),
        })
        .spawn();
    }
    Ok(())
}

fn page_hit_rate_monitor_thread_fn(
    prefetcher_registration_observer: Box<dyn StrongObserver<PageCacheRegistrationCommand>>,
    print_timer_table_consumer: Box<dyn Consumer<()>>,
) -> ! {
    let mut access_observer = None;
    let registration_waiter = new_waiter();
    let print_timer_waiter = new_waiter();
    let access_waiter = new_waiter();

    let mut hit_count: u64 = 0;
    let mut total_count: u64 = 0;

    loop {
        let wake_index = Waiter::wait_many_iter(
            [&registration_waiter, &access_waiter, &print_timer_waiter].into_iter(),
        );
        match wake_index {
            0 => {
                let mut register = || {
                    prefetcher_registration_observer
                        .enqueue_for_strong_observe(registration_waiter.waker());
                    if let Some(reg) = prefetcher_registration_observer.try_strong_observe() {
                        println!("start_page_hit_rate_monitor: registered new page cache (discarding the old one)");
                        access_waiter.waker().wake_up();
                        access_observer = Some(reg.access_table.attach_strong_observer()?);
                        hit_count = 0;
                        total_count = 0;
                    }
                    Ok::<_, Box<dyn core::error::Error>>(())
                };
                error_result!(register())
            }
            1 => {
                if let Some(access_observer) = &access_observer {
                    access_observer.enqueue_for_strong_observe(access_waiter.waker());
                    if let Some(event) = access_observer.try_strong_observe() {
                        if event.is_cache_hit {
                            hit_count += 1;
                        }
                        total_count += 1;
                    }
                }
            }
            2 => {
                print_timer_table_consumer.enqueue_for_take(print_timer_waiter.waker());
                if let Some(()) = print_timer_table_consumer.try_take() {
                    let hit_count: fixed::types::U64F64 = hit_count.into();
                    let total_count: fixed::types::U64F64 = total_count.into();
                    if total_count > 0 {
                        println!("Page cache hit rate: {}", hit_count / total_count);
                    } else {
                        println!("Page cache hit rate: no events");
                    }
                }
            }
            _ => unreachable!(),
        }
    }
}

fn new_waiter() -> Waiter {
    let (waiter, _) = Waiter::new_pair();
    waiter.waker().wake_up();
    waiter
}

pub struct BufferedBlockDeviceWriter {
    device: Arc<dyn BlockDevice>,
    offset: usize,
    buffer: Vec<u8>,
}

impl Write for BufferedBlockDeviceWriter {
    fn write(&mut self, buf: &[u8]) -> core2::io::Result<usize> {
        // TODO: A bunch of error information is discarded.
        self.buffer.extend_from_slice(buf);
        self.write_available()
            .map_err(|_| core2::io::Error::new(core2::io::ErrorKind::InvalidInput, "Unknown"))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> core2::io::Result<()> {
        // TODO: A bunch of error information is discarded.
        self.write_available()
            .map_err(|_| core2::io::Error::new(core2::io::ErrorKind::InvalidInput, "Unknown"))?;
        self.buffer.extend(
            (0..(SECTOR_SIZE - (self.buffer.len() % SECTOR_SIZE)) % SECTOR_SIZE).map(|_| 0),
        );
        assert!(self.buffer.len() % SECTOR_SIZE == 0);
        let _ = self
            .device
            .write_bytes_async(self.offset, &self.buffer)
            .map_err(|_| core2::io::Error::new(core2::io::ErrorKind::InvalidInput, "Unknown"))?;
        Ok(())
    }
}

impl BufferedBlockDeviceWriter {
    pub fn new(device: Arc<dyn BlockDevice>, offset: usize) -> Self {
        Self {
            device,
            offset,
            buffer: Default::default(),
        }
    }

    fn write_available(&mut self) -> ostd::Result<()> {
        let available_bytes = self.buffer.len() - (self.buffer.len() % SECTOR_SIZE);
        if available_bytes == 0 {
            return Ok(());
        }
        let buf = &self.buffer[0..available_bytes];
        let _ = self.device.write_bytes_async(self.offset, buf)?;
        self.offset += buf.len();
        self.buffer.drain(0..buf.len());
        Ok(())
    }
}
