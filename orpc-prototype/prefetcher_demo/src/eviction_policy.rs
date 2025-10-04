use std::{collections::VecDeque, error::Error};

use orpc::{
    RPCError, ServerRef,
    oqueue::{OQueueRef, locking::LockingQueue},
    orpc_impl, orpc_server, spawn_thread,
    sync::Mutex,
};

use crate::block_device::{EvictionPolicy, IOError};

#[orpc_server(crate::block_device::EvictionPolicy)]
pub struct LeastRecentlyUsed {
    blocks_in_cache: Mutex<VecDeque<usize>>,
    insertion_oqueue: OQueueRef<usize>,
}

#[orpc_impl]
impl EvictionPolicy for LeastRecentlyUsed {
    fn select_victim(&self) -> Result<usize, IOError> {
        if let Some(victim_id) = self.blocks_in_cache.lock().pop_front() {
            Ok(victim_id)
        } else {
            Err(IOError::IOFailure {
                message: "No block to evict".to_owned(),
            })
        }
    }

    fn inserted(&self, block_id: usize) -> Result<(), RPCError> {
        self.insertion_oqueue
            .attach_sender()
            .map_err(|_| RPCError::ServerMissing)?
            .send(block_id);
        Ok(())
    }
}

impl LeastRecentlyUsed {
    pub fn spawn() -> Result<ServerRef<Self>, Box<dyn Error>> {
        let server = Self::new_with(|orpc_internal| Self {
            orpc_internal,
            blocks_in_cache: Default::default(),
            insertion_oqueue: LockingQueue::new(16),
        });

        spawn_thread(server.clone(), {
            let server = server.clone();
            let insertion_oqueue_receiver = server.insertion_oqueue.attach_receiver()?;
            move || {
                loop {
                    let block_id = insertion_oqueue_receiver.receive();
                    server.blocks_in_cache.lock().push_back(block_id);
                }
            }
        });

        Ok(server)
    }
}

#[orpc_server(crate::block_device::EvictionPolicy)]
pub struct Random {
    blocks_in_cache: Mutex<Vec<usize>>,
    insertion_oqueue: OQueueRef<usize>,
}

#[orpc_impl]
impl EvictionPolicy for Random {
    fn select_victim(&self) -> Result<usize, IOError> {
        if let Some(victim_id) = self
            .blocks_in_cache
            .lock()
            .remove_random(&mut ThreadRng::default())
        {
            Ok(victim_id)
        } else {
            Err(IOError::IOFailure {
                message: "No block to evict".to_owned(),
            })
        }
    }

    fn inserted(&self, block_id: usize) -> Result<(), RPCError> {
        self.insertion_oqueue
            .attach_sender()
            .map_err(|_| RPCError::ServerMissing)?
            .send(block_id);
        Ok(())
    }
}

impl Random {
    pub fn spawn() -> Result<ServerRef<Self>, Box<dyn Error>> {
        let server = Self::new_with(|orpc_internal| Self {
            orpc_internal,
            blocks_in_cache: Default::default(),
            insertion_oqueue: LockingQueue::new(16),
        });

        spawn_thread(server.clone(), {
            let server = server.clone();
            let insertion_oqueue_receiver = server.insertion_oqueue.attach_receiver()?;
            move || {
                loop {
                    let block_id = insertion_oqueue_receiver.receive();
                    server.blocks_in_cache.lock().push(block_id);
                }
            }
        });

        Ok(server)
    }
}

use rand::{Rng, rngs::ThreadRng};

pub trait RemoveRandom {
    type Item;

    fn remove_random<R: Rng>(&mut self, rng: &mut R) -> Option<Self::Item>;
}

impl<T> RemoveRandom for Vec<T> {
    type Item = T;

    fn remove_random<R: Rng>(&mut self, rng: &mut R) -> Option<Self::Item> {
        if self.len() == 0 {
            None
        } else {
            let index = rng.random_range(0..self.len());
            Some(self.swap_remove(index))
        }
    }
}
