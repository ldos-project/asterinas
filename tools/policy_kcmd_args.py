#!/usr/bin/env python3
# SPDX-License-Identifier: MPL-2.0

"""Prints the kernel command-line parameters set by a policy config (see `policies/default.toml`)."""

import sys
import tomllib


def kcmd_args(table, prefix=""):
    for key, value in table.items():
        name = prefix + key
        if isinstance(value, dict):
            yield from kcmd_args(value, name + ".")
        elif isinstance(value, list):
            yield f"{name}=" + ",".join(map(str, value))
        elif isinstance(value, bool):
            # TOML `true` must reach the kernel as `true`, not Python's `True`.
            yield f"{name}={str(value).lower()}"
        else:
            yield f"{name}={value}"


with open(sys.argv[1], "rb") as config:
    for arg in kcmd_args(tomllib.load(config)):
        print(arg)
