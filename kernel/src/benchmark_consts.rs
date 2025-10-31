use alloc::sync::Arc;

use ostd::orpc::oqueue::OQueue;

use super::*;

pub const N_MESSAGES_PER_THREAD: usize = 2 << 15;
pub struct BenchConsts {
    pub n_threads: usize,
    pub n_messages: usize,
    pub q_type: bool,
    pub benchmark: String,
}
impl BenchConsts {
    pub fn new(karg: &super::KCmdlineArg) -> Self {
        let n_threads = karg
            .get_module_arg_by_name::<usize>("bench", "n_threads")
            .unwrap();
        Self {
            n_threads,
            n_messages: N_MESSAGES_PER_THREAD * n_threads,
            q_type: karg
                .get_module_arg_by_name::<String>("bench", "q_type")
                .unwrap()
                == "mpmc_oq",
            benchmark: karg
                .get_module_arg_by_name::<String>("bench", "benchmark")
                .unwrap(),
        }
    }

    pub fn get_oq(&self) -> Arc<dyn OQueue<u64>> {
        if self.q_type {
            let q = ostd::orpc::oqueue::ringbuffer::mpmc::MPMCOQueue::<u64>::new(2 << 20, 16);
            assert!(q.capacity() >= self.n_messages);
            let q: Arc<dyn OQueue<u64>> = q;
            q
        } else {
            let q = ostd::orpc::oqueue::ringbuffer::mpmc::Rigtorp::<u64>::new(2 << 20);
            assert!(q.capacity() >= self.n_messages);
            let q: Arc<dyn OQueue<u64>> = q;
            q
        }
    }

    pub fn run_benchmark(&self) {
        let benchmark = match self.benchmark.as_str() {
            "mixed_bench" => benchmarks::mixed_bench,
            "consume_bench" => benchmarks::consume_bench,
            "produce_bench" => benchmarks::produce_bench,
            "weak_obs_bench" => benchmarks::weak_obs_bench,
            _ => panic!("Unknown benchmark"),
        };
        for _ in 0..10 {
            let now = time::clocks::RealTimeClock::get().read_time();

            let q: Arc<dyn OQueue<u64>> = self.get_oq();

            let completed = Arc::new(AtomicUsize::new(0));
            let completed_wq = Arc::new(ostd::sync::WaitQueue::new());
            let n_threads = benchmark(self, &q, &completed, &completed_wq);

            println!("Waiting for benchmark to complete");
            // Exit after benchmark completes
            completed_wq
                .wait_until(|| (completed.load(Ordering::Relaxed) == n_threads).then_some(()));
            let end = time::clocks::RealTimeClock::get().read_time();

            println!("[total] {:?}", end - now);
        }
    }
}
