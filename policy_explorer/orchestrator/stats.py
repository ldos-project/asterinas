# SPDX-License-Identifier: MPL-2.0

"""Statistics for subsystems' `criteria`."""

from scipy import stats


def means(runs: list[dict]) -> dict:
    """Mean of each metric over runs."""
    keys = runs[0].keys()
    return {key: sum(run[key] for run in runs) / len(runs) for key in keys}


def welch(candidate_runs: list[dict], baseline_runs: list[dict], keys, *, lower_is_better=True, alpha=0.05) -> dict:
    """Test, per metric, whether the candidate beats the baseline (one-sided Welch t-test)."""
    result = {}
    for key in keys:
        # Skip metrics missing on either side (a run recorded under an older metric schema).
        if key not in candidate_runs[0] or key not in baseline_runs[0]:
            continue
        candidate = [run[key] for run in candidate_runs]
        baseline = [run[key] for run in baseline_runs]
        alternative = "less" if lower_is_better else "greater"
        _, p_value = stats.ttest_ind(candidate, baseline, equal_var=False, alternative=alternative)
        result[key] = {"p_value": float(p_value), "significant": bool(p_value < alpha)}
    return result
