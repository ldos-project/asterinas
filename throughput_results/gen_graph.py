thread_counts = [1, 2, 4, 8, 16, 32]


def to_ns(s: str) -> float:
    v = float(s[:-2])
    suffix = s[-2:]
    if suffix == "ns":
        v *= 1
    elif suffix == "μs":
        v *= 1000
    elif suffix == "ms":
        v *= 1000 * 1000
    else:
        v = float(s[:-1])
        v *= 1000 * 1000 * 1000
    return v


mpmc_oq_produce_tp = []
rigtorp_produce_tp = []

for tc in thread_counts:
    with open(f"mpmc_oq_produce_throughput_{tc}.log") as f:
        values = [
            to_ns(line.split()[-1])
            for line in f.readlines()
            if line.startswith("[producer")
        ]
        assert len(values) == tc
        mpmc_oq_produce_tp.append(values)
print(mpmc_oq_produce_tp)
