//! An ORPC server implementing a block cache

use std::{collections::HashMap, error::Error};

use orpc::{
    CurrentServer, ServerRef,
    oqueue::{OQueueRef, Receiver, Sender, locking::LockingQueue},
    orpc_impl, orpc_server, spawn_thread,
    sync::{Mutex, blocker::select},
};

use crate::block_device::{Block, BlockCache, BlockDevice, EvictionPolicy, IOError, ReadRequest};

#[orpc_server(crate::block_device::BlockDevice, crate::block_device::BlockCache)]
pub struct Cache {
    cache: Mutex<HashMap<usize, Box<[u8]>>>,
    reply_oqueue: OQueueRef<Block>,
    underlying: ServerRef<dyn BlockDevice>,
    cache_size: usize,
    eviction_policy: ServerRef<dyn EvictionPolicy>,
}

#[orpc_impl]
impl BlockDevice for Cache {
    // TODO: It should be possible to put this as a default implementation in the trait.
    fn read(&self, block_id: usize) -> Result<Block, IOError> {
        self.read_request_observation_oqueue()
            .attach_sender()?
            .send(block_id);
        {
            let cache = self.cache.lock();
            if let Some(data) = cache.get(&block_id).cloned() {
                println!("HIT {} (Cache)", block_id);
                return Ok(Block { block_id, data });
            }
        }
        println!("MISS {} (Cache)", block_id);
        let block = self.underlying.read(block_id)?;
        self.insert_cached_block(&block);
        Ok(block)
    }

    fn read_request_observation_oqueue(&self) -> OQueueRef<usize>;

    fn read_request_oqueue(&self) -> OQueueRef<ReadRequest>;
}

#[orpc_impl]
impl BlockCache for Cache {
    fn prefetch_request_oqueue(&self) -> OQueueRef<usize>;
}

impl Cache {
    fn insert_cached_block(&self, block: &Block) {
        let mut cache = self.cache.lock();
        if cache.len() >= self.cache_size {
            self.evict_block(&mut cache);
        }

        // If we still don't have space, do not insert into cache at all.
        if cache.len() >= self.cache_size {
            println!("    Cache insertion failed {} (Cache)", block.block_id);
            return;
        }

        println!("    Inserting {} (Cache)", block.block_id);
        match cache.entry(block.block_id) {
            std::collections::hash_map::Entry::Occupied(_) => {}
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                if let Err(e) = self.eviction_policy.inserted(block.block_id) {
                    println!(
                        "leaked block {}. It may never be evicted. {}",
                        block.block_id, e
                    );
                }
                vacant_entry.insert(block.data.clone());
            }
        }
    }

    fn evict_block(&self, cache: &mut HashMap<usize, Box<[u8]>>) {
        match self.eviction_policy.select_victim() {
            Ok(k) => {
                // println!("    Evicting {} (Cache)", k);
                cache.remove(&k);
            }
            Err(e) => {
                println!("eviction error: {}", e);
            }
        }
    }

    fn send_underlying_request(
        &self,
        underlying_read_request_sender: &dyn Sender<ReadRequest>,
        block_id: usize,
        outstanding_requests: &mut HashMap<usize, Vec<Box<dyn Sender<Block>>>>,
    ) {
        if !outstanding_requests.contains_key(&block_id)
            && !self.cache.lock().contains_key(&block_id)
        {
            underlying_read_request_sender.send(ReadRequest {
                reply_to: self.reply_oqueue.attach_sender().unwrap(),
                block_id,
            });
        }
    }

    fn handle_request(
        &self,
        underlying_read_request_sender: &dyn Sender<ReadRequest>,
        req: ReadRequest,
        outstanding_requests: &mut HashMap<usize, Vec<Box<dyn Sender<Block>>>>,
    ) {
        {
            let cache = self.cache.lock();
            if let Some(data) = cache.get(&req.block_id).cloned() {
                println!("HIT {} (async)", req.block_id);
                req.reply_to.send(Block {
                    block_id: req.block_id,
                    data,
                });
                return;
            }
        }

        println!("MISS {} (async)", req.block_id);
        let entry = outstanding_requests.entry(req.block_id);
        let waiting = entry.or_insert_with(Default::default);
        waiting.push(req.reply_to);
        self.send_underlying_request(
            underlying_read_request_sender,
            req.block_id,
            outstanding_requests,
        );
    }

    fn handle_reply(
        &self,
        block: Block,
        outstanding_requests: &mut HashMap<usize, Vec<Box<dyn Sender<Block>>>>,
    ) {
        // println!("Cache: Message read reply {}", block.block_id);
        self.insert_cached_block(&block);
        if let Some(waiters) = outstanding_requests.remove(&block.block_id) {
            for waiter in waiters {
                waiter.send(block.clone());
            }
        }
    }

    fn read_handler_thread(
        &self,
        underlying_read_request_sender: Box<dyn Sender<ReadRequest>>,
        reply_receiver: Box<dyn Receiver<Block>>,
        read_request_receiver: Box<dyn Receiver<ReadRequest>>,
        prefetch_request_receiver: Box<dyn Receiver<usize>>,
    ) {
        let mut outstanding_requests = HashMap::new();

        let read_request_observation_sender = self
            .read_request_observation_oqueue()
            .attach_sender()
            .unwrap();
        loop {
            select!(
                if let req = read_request_receiver.try_receive() {
                    read_request_observation_sender.send(req.block_id);
                    self.handle_request(
                        underlying_read_request_sender.as_ref(),
                        req,
                        &mut outstanding_requests,
                    );
                },
                if let req = prefetch_request_receiver.try_receive() {
                    println!("  Prefetch req {}", req);
                    self.send_underlying_request(
                        underlying_read_request_sender.as_ref(),
                        req,
                        &mut outstanding_requests,
                    );
                },
                if let req = reply_receiver.try_receive() {
                    self.handle_reply(req, &mut outstanding_requests);
                },
            );
            CurrentServer::abort_point();
        }
    }

    pub fn spawn(
        underlying: ServerRef<dyn BlockDevice>,
        eviction_policy: ServerRef<dyn EvictionPolicy>,
        cache_size: usize,
    ) -> Result<ServerRef<Self>, Box<dyn Error>> {
        let server = Self::new_with(|orpc_internal| Self {
            orpc_internal,
            cache_size,
            cache: Default::default(),
            reply_oqueue: LockingQueue::new(16),
            underlying: underlying.clone(),
            eviction_policy: eviction_policy.clone(),
        });
        spawn_thread(server.clone(), {
            let server = server.clone();
            let reply_receiver = server.reply_oqueue.attach_receiver()?;
            let underlying_read_request_sender =
                underlying.read_request_oqueue().attach_sender()?;
            let read_request_receiver = server.read_request_oqueue().attach_receiver()?;
            let prefetch_request_receiver = server.prefetch_request_oqueue().attach_receiver()?;
            move || {
                server.read_handler_thread(
                    underlying_read_request_sender,
                    reply_receiver,
                    read_request_receiver,
                    prefetch_request_receiver,
                );
                Ok(())
            }
        });
        Ok(server)
    }
}
