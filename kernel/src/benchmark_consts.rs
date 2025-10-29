use ostd::orpc::oqueue::OQueue;

pub const N_THREADS: usize = 32;
pub const N_MESSAGES_PER_THREAD: usize = 2 << 15;
pub const N_MESSAGES: usize = N_MESSAGES_PER_THREAD * N_PRODUCERS;

pub fn get_oq() -> Arc<dyn OQueue<u64> {
    let q = ostd::orpc::oqueue::ringbuffer::MPMCOQueue::<u64>::new(2 << 20, 0);
    // let q = ostd::orpc::oqueue::ringbuffer::mpmc::Rigtorp::<u64>::new(2 << 20);
    let q: Arc<dyn OQueue<u64>> = q;
    q
}
