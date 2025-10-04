#![feature(trait_upcasting)]
#![allow(unused)]

use std::{
    env,
    error::Error,
    fs::OpenOptions,
    io::{Seek, Write, stdin},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicIsize},
    },
    thread::{self, sleep},
    time::Duration,
};

use egui::{CentralPanel, Context, Slider};
use orpc::{
    ServerRef,
    oqueue::{
        OQueueRef,
        locking::{LockingQueue, ObservableLockingQueue},
        registry::register,
    },
};

use crate::{
    block_device::{BLOCK_SIZE, BlockCache, BlockDevice},
    shutdown::Shutdown,
};

extern crate alloc;

mod block_device;
mod cache;
mod disk;
mod eviction_policy;
mod prefetch_policy;
pub(crate) mod shutdown;

const N_BLOCKS: usize = 1024;

fn main() {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "Prefetcher Demo",
        options,
        Box::new(|_cc| Ok(Box::new(PrefetcherDemo::new().unwrap()))),
    );
}

struct DemoServers {
    disk: ServerRef<dyn BlockDevice>,
    cache: ServerRef<dyn BlockCache>,
    policy: Option<ServerRef<dyn Shutdown>>,
    system_load_oqueue: OQueueRef<u8>,
    stride: Arc<AtomicIsize>,
    running: Arc<AtomicBool>,
}

impl DemoServers {
    fn new() -> Result<Self, Box<dyn Error>> {
        let block_file = setup_block_file();

        let system_load_oqueue: OQueueRef<u8> = ObservableLockingQueue::new(2, 2);
        register("system_load", system_load_oqueue.clone());

        // Start the disk server
        let disk = disk::Disk::spawn(block_file)?;

        // Start the eviction policy
        let eviction_policy = eviction_policy::Random::spawn()?;

        // Start the cache server attached to the disk.
        let cache = cache::Cache::spawn(disk.clone(), eviction_policy, 16)?;

        let ret = Self {
            disk,
            cache,
            policy: None,
            system_load_oqueue,
            stride: Arc::new(AtomicIsize::new(1)),
            running: Arc::new(AtomicBool::new(true)),
        };

        ret.start_simulation();

        Ok(ret)
    }

    fn start_simulation(&self) {
        // A client (would be a server in the real thing)
        let client = thread::spawn({
            let cache = self.cache.clone();
            let stride = self.stride.clone();
            let running = self.running.clone();
            move || {
                let mut i = 0;
                loop {
                    if running.load(std::sync::atomic::Ordering::Relaxed) {
                        cache
                            .read(
                                (i * stride.load(std::sync::atomic::Ordering::Relaxed)) as usize
                                    % N_BLOCKS,
                            )
                            .unwrap();
                        i += 1;
                    }
                    sleep(Duration::from_secs(1));
                }
            }
        });
    }
}

/// Setup a file which stands in for a block device.
fn setup_block_file() -> std::fs::File {
    let mut block_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open("/tmp/block_tmp_file")
        .unwrap();
    block_file
        .seek(std::io::SeekFrom::Current((N_BLOCKS * BLOCK_SIZE) as i64))
        .unwrap();
    block_file.write(&[0]).unwrap();
    block_file
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PrefetchPolicyKind {
    None,
    Next,
    Strided,
    LoadAware,
}

struct PrefetcherDemo {
    servers: DemoServers,
    policy: PrefetchPolicyKind,
    system_load: u8,
    stride: Arc<AtomicIsize>,
    running: bool,
}

impl PrefetcherDemo {
    fn new() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            servers: DemoServers::new()?,
            system_load: 128,
            policy: PrefetchPolicyKind::None,
            stride: Arc::new(AtomicIsize::new(3)),
            running: true,
        })
    }
}

impl eframe::App for PrefetcherDemo {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        CentralPanel::default().show(ctx, |ui| {
            ui.heading("Prefetcher Demo");

            ui.toggle_value(&mut self.running, "Running");
            self.servers
                .running
                .store(self.running, std::sync::atomic::Ordering::Relaxed);

            let mut stride = self
                .servers
                .stride
                .load(std::sync::atomic::Ordering::Relaxed);
            ui.label("Stride:");
            if ui.add(Slider::new(&mut stride, 0..=10)).changed() {
                self.servers
                    .stride
                    .store(stride, std::sync::atomic::Ordering::Relaxed);
            }

            let mut policy = self.policy;
            ui.horizontal(|ui| {
                ui.radio_value(&mut policy, PrefetchPolicyKind::None, "None");
                ui.radio_value(&mut policy, PrefetchPolicyKind::Next, "Next");
                ui.radio_value(&mut policy, PrefetchPolicyKind::Strided, "Strided");
                ui.radio_value(&mut policy, PrefetchPolicyKind::LoadAware, "Load Aware");
            });

            if self.policy != policy {
                if let Some(p) = &self.servers.policy {
                    p.shutdown();
                }
                match policy {
                    PrefetchPolicyKind::None => {
                        self.servers.policy = None;
                    }
                    PrefetchPolicyKind::Next => {
                        let p =
                            prefetch_policy::NextPagePrefetcher::spawn(self.servers.cache.clone())
                                .ok();
                        self.servers.policy = p.map(|v| v as _);
                    }
                    PrefetchPolicyKind::Strided => {
                        let p =
                            prefetch_policy::StridePrefetcher::spawn(self.servers.cache.clone())
                                .ok();
                        self.servers.policy = p.map(|v| v as _);
                    }
                    PrefetchPolicyKind::LoadAware => {
                        let p = prefetch_policy::StrideLoadAwarePrefetcher::spawn(
                            self.servers.cache.clone(),
                        )
                        .ok();
                        self.servers.policy = p.map(|v| v as _);
                    }
                }
                self.policy = policy;
            }

            ui.label("System Load:");
            if ui
                .add(Slider::new(&mut self.system_load, 0..=255))
                .changed()
            {
                self.servers
                    .system_load_oqueue
                    .attach_sender()
                    .unwrap()
                    .send(self.system_load);
            }
        });
    }
}
