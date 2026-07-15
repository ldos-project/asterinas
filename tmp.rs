fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
    let reply_handle = self.bio_completion_oqueue().attach_ref_producer()?;

    let mut bio = bio;
    let device_index = self.device.device_index.load(Ordering::Relaxed);
    bio.prepare_enqueue(reply_handle, device_index);
    self.device.inc_page_counter(bio.num_pages());
    let producer = self.bio_submission_oqueue().attach_value_producer()?;
    producer.produce(bio);
    Ok(())
}

bio.prepare_enqueue(reply_handle, device_index, self.device.num_outstanding_pages.load(Ordering::Relaxed));



fn num_outstanding_pages(&self) -> u64 {
    self.device.num_outstanding_pages.load(Ordering::Relaxed)
}


// Refactor the policy initialization to somewhere else. 

let mut members = Vec::with_capacity(RAID_MEMBER_NAMES.len());

// Start the RAID-1's underlying member devices.
for (index, &name) in RAID_MEMBER_NAMES.iter().enumerate() {
    match start_block_device(name) {
        Ok(device) => {
            info!("[raid] member '{}' online", name);
            if let Some(virtio_dev) = device.downcast_ref::<VirtIoBlockDevice>() {
                virtio_dev.set_device_index(index as u64);
            }
            members.push(device);
        }
        Err(err) => {
            error!(
                "[raid] failed to start member '{}': {:?}. RAID-1 init aborted",
                name, err
            );
            return Err(err);
        }
    }
}

#[cfg(not(baseline_asterinas))]
info!("[raid] creating selection policy");

// Round Robin Policy
#[cfg(all(not(baseline_asterinas), raid_selection = "roundrobin"))]
let selection_policy = RoundRobinPolicy::new(members.clone()).unwrap();

// Shared weak observer setup for all observer-based policies (LinnOS, LinnOS Plus, Decision Tree)
#[cfg(all(not(baseline_asterinas), any(raid_selection = "linnos", raid_selection = "linnos_plus", raid_selection = "decision_tree")))]
let observers = {
    use aster_virtio::device::block::server_traits::BlockIOObservable;
    use ostd::orpc::oqueue::{OQueueBase, ObservationQuery};
    members
        .iter()
        .map(|dev| {
            let virtio_dev = dev
                .downcast_ref::<VirtIoBlockDevice>()
                .expect("RAID member must be a VirtIoBlockDevice");
            ostd::sync::Mutex::new(
                virtio_dev
                    .bio_completion_oqueue()
                    .attach_weak_observer(4, ObservationQuery::identity())
                    .expect("Failed to attach weak observer to bio_completion_oqueue"),
            )
        })
        .collect()
};

// LinnOS Policy
#[cfg(all(not(baseline_asterinas), raid_selection = "linnos"))]
let selection_policy = LinnOSPolicy::new(members.clone(), observers).unwrap();

// LinnOS Plus Policy
#[cfg(all(not(baseline_asterinas), raid_selection = "linnos_plus"))]
let selection_policy = LinnOSPlusPolicy::new(members.clone(), observers).unwrap();

// Decision Tree Policy
#[cfg(all(not(baseline_asterinas), raid_selection = "decision_tree"))]
let selection_policy = DecisionTreePolicy::new(members.clone(), observers).unwrap();

// Initialize and Register RAID-1 device
#[cfg(not(baseline_asterinas))]
let raid1device = Raid1Device::init(raid_device_name, members, selection_policy);
#[cfg(baseline_asterinas)]
let raid1device = Raid1Device::init(raid_device_name, members);
raid1device.map_err(|err| match err {
    Raid1DeviceError::NotEnoughMembers => {
        Error::with_message(Errno::EINVAL, "RAID-1 device requires at least two members")
    }
})?;
info!("[raid] RAID-1 device created");

let worker = aster_block::get_device(raid_device_name).unwrap();
// The registry stores `Arc<dyn BlockDevice>`. Use `downcast_ref` on the captured Arc each
// iteration to call the RAID-specific helper without needing ownership of `Raid1Device`.
// TODO(Yingqi): Merge the starting of the RAID-1 thread inside block device server.
let task_fn = move || {
    info!("spawn the RAID-1 device thread");
    let raid = worker.downcast_ref::<Raid1Device>().unwrap();
    loop {
        raid.handle_requests();
    }
};

let mut file_table_ref = ctx.thread_local.borrow_file_table_mut();
let mut file_table = file_table_ref.unwrap().write();

file_table.insert(Arc::new(stdin), FdFlags::empty());
file_table.insert(Arc::new(stdout), FdFlags::empty());
file_table.insert(Arc::new(stderr), FdFlags::empty());


-----------------------------------
fn enqueue(&self, bio: SubmittedBio) -> Result<(), BioEnqueueError> {
    let reply_handle = self.bio_completion_oqueue().attach_ref_producer()?;

    let mut bio = bio;
    let device_index = self.device.device_index.load(Ordering::Relaxed);
    bio.prepare_enqueue(reply_handle, device_index as u32, self.device.num_outstanding_pages.load(Ordering::Relaxed) as u32, self.device.num_outstanding_requests.load(Ordering::Relaxed) as u32);
    self.device.inc_page_counter(bio.num_pages());
    self.device.inc_request_counter();
    // log::info!("\x1b[32mIncremented\x1b[0m Page Counter by {}, new value: {}, device_index: {}, type: {:?}", bio.num_pages(), self.device.num_outstanding_pages.load(Ordering::Relaxed), device_index, bio.type_());
    let producer = self.bio_submission_oqueue().attach_value_producer()?;
    producer.produce(bio);
    Ok(())
}

fn num_outstanding_pages(&self) -> u32 {
    self.device.num_outstanding_pages.load(Ordering::Relaxed)
}

fn num_outstanding_requests(&self) -> u32 {
    self.device.num_outstanding_requests.load(Ordering::Relaxed)
}




fn setup_raid1_device(raid_device_name: &str) -> Result<()> {
    const RAID_MEMBER_NAMES: &[&str] = &["raid0", "raid1", "raid2"];
    info!(
        "[raid] initializing RAID-1 '{}' with members {:?}",
        raid_device_name, RAID_MEMBER_NAMES
    );

    let mut members = Vec::with_capacity(RAID_MEMBER_NAMES.len());

    // Start the RAID-1's underlying member devices.
    for (index, &name) in RAID_MEMBER_NAMES.iter().enumerate() {
        match start_block_device(name) {
            Ok(device) => {
                info!("[raid] member '{}' online", name);
                if let Some(virtio_dev) = device.downcast_ref::<VirtIoBlockDevice>() {
                    virtio_dev.set_device_index((index) as u32);
                }
                members.push(device);
            }
            Err(err) => {
                error!(
                    "[raid] failed to start member '{}': {:?}. RAID-1 init aborted",
                    name, err
                );
                return Err(err);
            }
        }
    }

    #[cfg(all(not(baseline_asterinas), capture_data))]
    setup_data_capture(&members, RAID_MEMBER_NAMES);

    // Clone members for Heimdall before they are consumed by the selection policy / RAID init.
    #[cfg(not(baseline_asterinas))]
    let members_for_heimdall = members.clone();

    // Initialize Heimdall device performance monitor
    #[cfg(not(baseline_asterinas))]
    {
        use aster_virtio::device::block::server_traits::BlockIOObservable;
        use ostd::orpc::oqueue::{OQueueBase, ObservationQuery};

        let heimdall_observers: Vec<_> = members_for_heimdall
            .iter()
            .map(|dev| {
                let virtio_dev = dev
                    .downcast_ref::<VirtIoBlockDevice>()
                    .expect("RAID member must be a VirtIoBlockDevice");
                virtio_dev
                    .bio_completion_oqueue()
                    .attach_strong_observer(ObservationQuery::identity())
                    .expect("Failed to attach strong observer for Heimdall")
            })
            .collect();

        let heimdall = aster_raid::heimdall::Heimdall::new(
            members_for_heimdall,
            heimdall_observers,
        )
        .expect("Failed to create Heimdall monitor");

        let heimdall_task = move || {
            info!("[heimdall] Heimdall monitor thread started");
            heimdall.run();
        };

        crate::ThreadOptions::new(heimdall_task)
            .sched_policy(crate::sched::SchedPolicy::RealTime {
                rt_prio: 50.try_into().unwrap(),
                rt_policy: crate::sched::RealTimePolicy::RoundRobin {
                    base_slice_factor: None,
                },
            })
            .spawn();

        info!("[heimdall] Heimdall monitor initialized and thread spawned");
    }

    

    #[cfg(not(baseline_asterinas))]
    info!("[raid] creating selection policy");

    // Shared weak observer setup for all observer-based policies (LinnOS, LinnOS Plus, Decision Tree)
    #[cfg(all(not(baseline_asterinas), any(raid_selection = "linnos", raid_selection = "linnos_plus", raid_selection = "decision_tree")))]
    let observers = {
        use aster_virtio::device::block::server_traits::BlockIOObservable;
        use ostd::orpc::oqueue::{OQueueBase, ObservationQuery};
        members
            .iter()
            .map(|dev| {
                let virtio_dev = dev
                    .downcast_ref::<VirtIoBlockDevice>()
                    .expect("RAID member must be a VirtIoBlockDevice");
                ostd::sync::Mutex::new(
                    virtio_dev
                        .bio_completion_oqueue()
                        .attach_weak_observer(4, ObservationQuery::identity())
                        .expect("Failed to attach weak observer to bio_completion_oqueue"),
                )
            })
            .collect()
    };

    // LinnOS Policy
    #[cfg(all(not(baseline_asterinas), raid_selection = "linnos"))]
    let selection_policy = LinnOSPolicy::new(members.clone(), observers).unwrap();

    // LinnOS Plus Policy
    #[cfg(all(not(baseline_asterinas), raid_selection = "linnos_plus"))]
    let selection_policy = LinnOSPlusPolicy::new(members.clone(), observers).unwrap();

    // Decision Tree Policy
    #[cfg(all(not(baseline_asterinas), raid_selection = "decision_tree"))]
    let selection_policy = DecisionTreePolicy::new(members.clone(), observers).unwrap();

    // Round Robin Policy (explicit or default when no raid_selection is specified)
    #[cfg(all(not(baseline_asterinas), any(raid_selection = "roundrobin", not(any(raid_selection = "linnos", raid_selection = "linnos_plus", raid_selection = "decision_tree")))))]
    let selection_policy = RoundRobinPolicy::new(members.clone()).unwrap();

    // Initialize and Register RAID-1 device
    #[cfg(not(baseline_asterinas))]
    let raid1device = Raid1Device::init(raid_device_name, members, selection_policy);
    #[cfg(baseline_asterinas)]
    let raid1device = Raid1Device::init(raid_device_name, members);
    raid1device.map_err(|err| match err {
        Raid1DeviceError::NotEnoughMembers => {
            Error::with_message(Errno::EINVAL, "RAID-1 device requires at least two members")
        }
    })?;
    info!("[raid] RAID-1 device created");

    let worker = aster_block::get_device(raid_device_name).unwrap();
    // The registry stores `Arc<dyn BlockDevice>`. Use `downcast_ref` on the captured Arc each
    // iteration to call the RAID-specific helper without needing ownership of `Raid1Device`.
    // TODO(Yingqi): Merge the starting of the RAID-1 thread inside block device server.
    let task_fn = move || {
        info!("spawn the RAID-1 device thread");
        let raid = worker.downcast_ref::<Raid1Device>().unwrap();
        loop {
            raid.handle_requests();
        }
    };

    crate::ThreadOptions::new(task_fn)
        .sched_policy(crate::sched::SchedPolicy::RealTime {
            rt_prio: 50.try_into().unwrap(),
            rt_policy: crate::sched::RealTimePolicy::RoundRobin {
                base_slice_factor: None,
            },
        })
        .spawn();

    info!(
        "[raid] RAID-1 device '{}' registered and worker thread spawned",
        raid_device_name
    );
    Ok(())
}

---