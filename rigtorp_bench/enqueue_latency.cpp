#define _GNU_SOURCE
#include <algorithm>
#include <atomic>
#include <chrono>
#include <iostream>
#include <numeric>
#include <pthread.h>
#include <rigtorp/MPMCQueue.h>
#include <sched.h>
#include <thread>
#include <unistd.h>
#include <vector>

using namespace std;

static const size_t QUEUE_SIZE = 1024;
static const size_t MESSAGES_PER_THREAD = 100'000; // 100k pushes per thread

// Utility: set current thread affinity to specific CPU
void pin_thread_to_cpu(size_t cpu_id) {
  cpu_set_t cpuset;
  CPU_ZERO(&cpuset);
  CPU_SET(cpu_id, &cpuset);
  int rc = pthread_setaffinity_np(pthread_self(), sizeof(cpu_set_t), &cpuset);
  if (rc != 0) {
    perror("pthread_setaffinity_np");
    exit(1);
  }
}

void consumer_thread(rigtorp::MPMCQueue<int> &q, atomic<bool> &done,
                     size_t cpu_id) {
  pin_thread_to_cpu(cpu_id);
  int val;
  while (!done.load(memory_order_relaxed)) {
    while (q.try_pop(val)) {
      // discard
    }
    this_thread::yield();
  }
  while (q.try_pop(val)) {
  }
}

vector<double> producer_thread(rigtorp::MPMCQueue<int> &q, size_t n_msgs,
                               size_t cpu_id) {
  pin_thread_to_cpu(cpu_id);

  vector<double> latencies;
  latencies.reserve(n_msgs);
  for (size_t i = 0; i < n_msgs; ++i) {
    auto start = chrono::high_resolution_clock::now();
    q.push(static_cast<int>(i));
    auto end = chrono::high_resolution_clock::now();
    double ns = chrono::duration_cast<chrono::nanoseconds>(end - start).count();
    latencies.push_back(ns);
  }
  return latencies;
}

void run_benchmark(size_t num_producers) {
  cout << "\n=== " << num_producers << " producer threads ===" << endl;

  size_t num_cores = thread::hardware_concurrency();
  if (num_producers + 1 > num_cores) {
    cerr << "Warning: more threads than CPU cores (" << num_cores << ")\n";
  }

  rigtorp::MPMCQueue<int> q(QUEUE_SIZE);
  atomic<bool> done = false;

  // Pin consumer to last core
  thread consumer([&] { consumer_thread(q, done, num_producers); });

  vector<thread> producers;
  vector<vector<double>> all_latencies(num_producers);

  auto start = chrono::steady_clock::now();

  for (size_t i = 0; i < num_producers; ++i) {
    producers.emplace_back([&, i]() {
      all_latencies[i] = producer_thread(q, MESSAGES_PER_THREAD, i);
    });
  }

  for (auto &t : producers)
    t.join();
  done = true;
  consumer.join();

  auto end = chrono::steady_clock::now();
  double seconds = chrono::duration<double>(end - start).count();

  // Merge all latencies
  vector<double> merged;
  for (auto &v : all_latencies)
    merged.insert(merged.end(), v.begin(), v.end());

  sort(merged.begin(), merged.end());
  double median = merged[merged.size() / 2];
  double p99 = merged[merged.size() * 99 / 100];
  double min_ns = merged.front();
  double max_ns = merged.back();
  double avg = accumulate(merged.begin(), merged.end(), 0.0) / merged.size();

  cout << "Pinned " << num_producers << " producers + 1 consumer across "
       << num_producers + 1 << " cores\n";
  cout << "Pushed " << merged.size() << " items total in " << seconds << " s\n";
  cout << "Min: " << min_ns << " ns, Median: " << median << " ns, P99: " << p99
       << " ns, Max: " << max_ns << " ns, Avg: " << avg << " ns\n";
}

int main() {
  cout << "Benchmarking rigtorp::MPMCQueue enqueue latency (with CPU "
          "affinity)\n";

  vector<size_t> thread_counts = {1, 2, 4, 8, 16};

  for (size_t t : thread_counts) {
    run_benchmark(t);
  }

  return 0;
}
