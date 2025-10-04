#![cfg(test)]

test_link_task_handle() {
    let fake = TaskHandle::Fake(0);
    match fake {
        Real(_) => error!("FAILED");
        Fake(x) => assert_eq!(x,0);
    }
}

