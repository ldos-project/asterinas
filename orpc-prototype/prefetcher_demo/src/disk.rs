//! An ORPC server emulating a disk.

use std::{
    fs::File,
    io::{self, Read, Seek},
};

use orpc::{
    CurrentServer, ServerRef,
    oqueue::{OQueue, OQueueRef, Receiver, locking::LockingQueue},
    orpc_impl, orpc_server, spawn_thread,
    sync::Mutex,
};
use snafu::{ResultExt as _, Whatever};

use crate::block_device::{BLOCK_SIZE, Block, BlockDevice, IOError, ReadRequest};

#[orpc_server(crate::block_device::BlockDevice)]
pub struct Disk {
    file: Mutex<File>,
}

#[orpc_impl]
impl BlockDevice for Disk {
    // TODO: It should be possible to put this as a default implementation in the trait.
    fn read(&self, block_id: usize) -> Result<Block, IOError> {
        // Report the call to the observers. THIS WILL BE AUTOMATIC IN AT LEAST SOME CASES.
        self.read_request_observation_oqueue()
            .attach_sender()?
            .send(block_id);
        // Create a reply queue
        let reply_oqueue = LockingQueue::new(2);
        let reply_receiver = reply_oqueue.attach_receiver()?;
        // Send the actual request.
        self.read_request_oqueue()
            .attach_sender()?
            .send(ReadRequest {
                block_id,
                reply_to: reply_oqueue.attach_sender()?,
            });
        // Wait for the reply.
        Ok(reply_receiver.receive())
    }

    fn read_request_observation_oqueue(&self) -> OQueueRef<usize>;

    fn read_request_oqueue(&self) -> OQueueRef<ReadRequest>;
}

impl Disk {
    /// Perform actual read.
    fn handle_request(&self, req: ReadRequest) -> Result<(), io::Error> {
        let mut file = self.file.lock();
        file.seek(std::io::SeekFrom::Start((req.block_id * BLOCK_SIZE) as u64))?;
        let mut data = vec![0; BLOCK_SIZE].into_boxed_slice();
        file.read_exact(&mut data)?;
        println!("      Read block {}", req.block_id);
        req.reply_to.send(Block {
            block_id: req.block_id,
            data,
        });
        Ok(())
    }

    fn read_handler_thread(&self, read_request_receiver: Box<dyn Receiver<ReadRequest>>) {
        let read_request_observation_sender = self
            .read_request_observation_oqueue()
            .attach_sender()
            .unwrap();
        loop {
            let req = read_request_receiver.receive();
            read_request_observation_sender.send(req.block_id);
            if let Err(e) = self.handle_request(req) {
                println!("IOError: {}", e);
            }
            CurrentServer::abort_point();
        }
    }

    pub fn spawn(file: File) -> Result<ServerRef<Self>, Whatever> {
        let server = Self::new_with(|orpc_internal| Self {
            file: Mutex::new(file),
            orpc_internal,
        });
        spawn_thread(server.clone(), {
            let server = server.clone();
            let read_request_receiver = server
                .read_request_oqueue()
                .attach_receiver()
                .whatever_context("read_request_oqueue")?;
            move || {
                server.read_handler_thread(read_request_receiver);
                Ok(())
            }
        });
        Ok(server)
    }
}
