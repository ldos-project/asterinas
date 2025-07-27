use alloc::{boxed::Box, vec::Vec};
use core::error::Error;

use ostd::{
    path,
    sync::Waiter,
    tables::{
        locking::LockingTable, registry::get_global_table_registry, Producer, Table, WeakObserver,
    },
    task::TaskOptions,
    timer::{self, Jiffies},
};

use crate::prelude::Pause;

const REGISTRATION_TABLE_BUFFER_SIZE: usize = 32;

#[derive(Clone, Copy, Debug)]
pub struct PrefetchCommand {
    pub page: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct PageAccessEvent {
    pub timestamp: usize,
    pub page: usize,
    pub access_type: AccessType,
}

#[derive(Clone, Copy, Debug)]
pub enum AccessType {
    Read,
    Write,
}

pub struct PageCacheRegistration {
    /// The table to send prefetch commands to.
    pub prefetch_command_producer: Box<dyn Producer<PrefetchCommand>>,
    /// The table to observe to see the pages accessed via this page cache.
    pub access_observer: Box<dyn WeakObserver<PageAccessEvent>>,
}

static_assertions::assert_impl_all!(PageCacheRegistration: Send);

fn start_prefetch_policy() -> Result<(), Box<dyn Error>> {
    let prefetcher_registration_table =
        LockingTable::<PageCacheRegistration>::new(REGISTRATION_TABLE_BUFFER_SIZE);
    let registry = get_global_table_registry();
    registry.register(
        path!(pagecache.policy.registration),
        prefetcher_registration_table.clone(),
    );

    let prefetch_registraction_consumer = prefetcher_registration_table.attach_consumer()?;

    TaskOptions::new(move || {
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
                    prefetch_registraction_consumer.enqueue_for_take(registration_waiter.waker());
                    if let Some(reg) = prefetch_registraction_consumer.try_take() {
                        registrations.push(reg);
                    }
                },

                1 => {
                    for PageCacheRegistration {
                        prefetch_command_producer,
                        access_observer,
                    } in registrations.iter()
                    {
                        // POLICY
                        let recent = access_observer.recent_cursor();
                        let observations =
                            (recent - 10..recent).flat_map(|i| access_observer.weak_observe(i));
                        if let Some(max) = observations
                            .map(
                                |PageAccessEvent {
                                     timestamp: _,
                                     page,
                                     access_type: _,
                                 }| page,
                            )
                            .max()
                        {
                            prefetch_command_producer.put(PrefetchCommand { page: max + 1 });
                        }
                    }
                    timer::wake_in(waiter.waker(), Jiffies::from_millis(100));
                },
                _ => unreachable!(),
            }
        }
    })
    .spawn()?;

    Ok(())
}
