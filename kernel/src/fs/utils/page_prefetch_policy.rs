use alloc::{boxed::Box, vec::Vec};
use core::error::Error;

use ostd::{
    path,
    tables::{
        locking::LockingTable, registry::get_global_table_registry, Producer, Table, WeakObserver
    }, task::TaskOptions,
};

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
    let prefetcher_registration_table = LockingTable::<PageCacheRegistration>::new(REGISTRATION_TABLE_BUFFER_SIZE); 
    let registry = get_global_table_registry();
    registry.register(path!(pagecache.policy.registration), prefetcher_registration_table.clone());

    let prefetch_registraction_consumer = prefetcher_registration_table.attach_consumer()?;

    TaskOptions::new(move || {
        let mut registrations = Vec::new();
        loop {
            if let Some(reg) = prefetch_registraction_consumer.try_take() {
                registrations.push(reg);
                continue;
            }
            for PageCacheRegistration { prefetch_command_producer, access_observer } in registrations.iter() {
                // POLICY
                let recent = access_observer.recent_cursor();
                let observations = (recent-10.. recent).flat_map(|i| access_observer.weak_observe(i));
                if let Some(max) = observations.map(|PageAccessEvent { timestamp: _, page, access_type: _ }| page).max() {
                    prefetch_command_producer.put(PrefetchCommand { page: max + 1 });
                }
            }
        }
    }).spawn()?;

    Ok(())
}
