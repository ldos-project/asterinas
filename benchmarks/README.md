# Benchmarking Framework

Over time we want to track the performance of benchmarks as we continue to improve the system.

## System Requirements

- Produces trackable metrics per benchmark
- Builds are hermetic
- Benchmarks are easy to add
- Metrics are easy to track

## System Architecture

```
benchmarks
├── flake.nix # This injects the state of what asterinas needs
├── rocksdb # Folder per benchmark
│   ├── flake.nix # Configuration for benchmark dependencies are inputs
│   ├── inputs # Configuration for benchmark dependencies are inputs
│   │   ├── small.sh # Defines a shell script for running -- use IN_ASTERINAS env var
│   │   └── big.sh
│   └── p90 # folder defines a trackable metric
│       └── filter.awk # awk filter script
└── ycsb-redis
    ├── flake.nix
    ├── inputs
    │   ├── small.sh
    │   └── big.sh
    └── p90
        └── filter.awk
```

We use nix because it's hermetic by design and so many of these benchmarks break as docker versions progress.
