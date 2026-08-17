#!/usr/bin/env python3
"""Welch t-test analysis for rocksdb mixgraph benchmark results.

Compares two OS variants from a single JSON file, or across two files.

Usage:
    python3 ttest_analysis.py result.json
    python3 ttest_analysis.py result.json --seeds 10
    python3 ttest_analysis.py result.json --a asterinas --b mariposa
    python3 ttest_analysis.py file1.json --compare file2.json
    python3 ttest_analysis.py file1.json --compare file2.json --a linux --b mariposa
    python3 ttest_analysis.py file1.json --compare file2.json --seeds 10
"""

import json
import math
import sys


def beta_cf(a, b, x, max_iter=200, eps=1e-14):
    qab = a + b
    qap = a + 1
    qam = a - 1
    d = 1.0 - qab * x / qap
    if abs(d) < 1e-30:
        d = 1e-30
    d = 1.0 / d
    h = d
    for m in range(1, max_iter + 1):
        m2 = 2 * m
        aa = m * (b - m) * x / ((qam + m2) * (a + m2))
        d = 1.0 + aa * d
        if abs(d) < 1e-30:
            d = 1e-30
        c = 1.0 + aa / c if m > 1 else 1.0 + aa
        if abs(c) < 1e-30:
            c = 1e-30
        d = 1.0 / d
        h *= d * c
        aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2))
        d = 1.0 + aa * d
        if abs(d) < 1e-30:
            d = 1e-30
        c = 1.0 + aa / c
        if abs(c) < 1e-30:
            c = 1e-30
        d = 1.0 / d
        delta = d * c
        h *= delta
        if abs(delta - 1.0) < eps:
            break
    return h


def beta_inc(a, b, x):
    if x == 0:
        return 0.0
    if x == 1:
        return 1.0
    bt = math.exp(
        math.lgamma(a + b) - math.lgamma(a) - math.lgamma(b)
        + a * math.log(x) + b * math.log(1 - x)
    )
    if x < (a + 1) / (a + b + 2):
        return bt * beta_cf(a, b, x) / a
    return 1.0 - bt * beta_cf(b, a, 1 - x) / b


def welch_ttest(a, b):
    n1, n2 = len(a), len(b)
    mean1 = sum(a) / n1
    mean2 = sum(b) / n2
    var1 = sum((x - mean1) ** 2 for x in a) / (n1 - 1) if n1 > 1 else 0
    var2 = sum((x - mean2) ** 2 for x in b) / (n2 - 1) if n2 > 1 else 0
    se = math.sqrt(var1 / n1 + var2 / n2)
    if se == 0:
        return mean1, mean2, 0.0, float("inf"), 1.0
    t = (mean1 - mean2) / se
    num = (var1 / n1 + var2 / n2) ** 2
    denom = (var1 / n1) ** 2 / (n1 - 1) + (var2 / n2) ** 2 / (n2 - 1)
    df = num / denom if denom > 0 else float("inf")
    if abs(t) < 0.001:
        return mean1, mean2, t, df, 1.0
    p = beta_inc(df / 2, 0.5, df / (df + t ** 2))
    return mean1, mean2, t, df, p


def significance(p):
    if p < 0.001:
        return "***"
    if p < 0.01:
        return "**"
    if p < 0.05:
        return "*"
    return "n.s."


def parse_args(argv):
    args = {"json_path": None, "compare_path": None, "max_seeds": None, "key_a": None, "key_b": None}
    positional = []
    i = 1
    while i < len(argv):
        if argv[i] == "--seeds":
            args["max_seeds"] = int(argv[i + 1])
            i += 2
        elif argv[i] == "--compare":
            args["compare_path"] = argv[i + 1]
            i += 2
        elif argv[i] == "--a":
            args["key_a"] = argv[i + 1]
            i += 2
        elif argv[i] == "--b":
            args["key_b"] = argv[i + 1]
            i += 2
        else:
            positional.append(argv[i])
            i += 1
    if positional:
        args["json_path"] = positional[0]
    return args


def main():
    args = parse_args(sys.argv)

    if args["json_path"] is None:
        print(
            "Usage: ttest_analysis.py <result.json> [--seeds N] [--a KEY --b KEY] [--compare file2.json]",
            file=sys.stderr,
        )
        sys.exit(1)

    with open(args["json_path"]) as f:
        data1 = json.load(f)

    compare_mode = args["compare_path"] is not None
    if compare_mode:
        with open(args["compare_path"]) as f:
            data2 = json.load(f)

    runs1 = data1["runs"]
    if args["max_seeds"] is not None:
        runs1 = runs1[: args["max_seeds"]]

    key_a = args["key_a"] or "linux"
    key_b = args["key_b"] or "asterinas"

    if compare_mode:
        runs2 = data2["runs"]
        if args["max_seeds"] is not None:
            runs2 = runs2[: args["max_seeds"]]
        label_a = key_a
        label_b = key_b
    else:
        label_a = key_a
        label_b = key_b

    stats_list = ["count", "avg", "stddev", "min", "median", "max", "p50", "p75", "p99", "p99_9", "p99_99"]
    ops = [("fill", "write"), ("mix", "read"), ("mix", "write"), ("mix", "seek")]

    total_sig = 0
    total_nonsig = 0

    for phase, op in ops:
        header = f"--- {phase}/{op} (N={len(runs1)} seeds) ---"
        print(header)
        print(
            "{:<12} {:>10} {:>10} {:>10} {:>8} {:<6}".format(
                "Stat", label_a, label_b, "t-stat", "p", "Sig"
            )
        )
        print("-" * 62)

        for stat in stats_list:
            if compare_mode:
                vals_a = []
                vals_b = []
                for r in runs1:
                    h = r["histogram"][key_a]
                    if h is not None:
                        vals_a.append(h[phase][op][stat])
                for r in runs2:
                    h = r["histogram"][key_b]
                    if h is not None:
                        vals_b.append(h[phase][op][stat])
            else:
                vals_a = []
                vals_b = []
                for r in runs1:
                    h = r["histogram"][key_a]
                    if h is not None:
                        vals_a.append(h[phase][op][stat])
                for r in runs1:
                    h = r["histogram"][key_b]
                    if h is not None:
                        vals_b.append(h[phase][op][stat])
            if len(vals_a) < 2 or len(vals_b) < 2:
                continue
            mean_a, mean_b, t, df, p = welch_ttest(vals_a, vals_b)
            sig = significance(p)
            if sig == "n.s.":
                total_nonsig += 1
            else:
                total_sig += 1
            print(
                "{:<12} {:>10.2f} {:>10.2f} {:>10.3f} {:>8.4f} {:<6}".format(
                    stat, mean_a, mean_b, t, p, sig
                )
            )
        print()

    print("=" * 62)
    print(f"Summary: {total_sig} significant, {total_nonsig} not significant")
    print("*** p<0.001  ** p<0.01  * p<0.05  n.s. not significant")
    print("Welch two-sample t-test (unequal variances)")
    if compare_mode:
        print(f"Comparing: {args['json_path']} ({key_a}) vs {args['compare_path']} ({key_b})")
    else:
        print(f"Comparing: {key_a} vs {key_b} from {args['json_path']}")


if __name__ == "__main__":
    main()
