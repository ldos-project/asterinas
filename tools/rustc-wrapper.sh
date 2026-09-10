#!/bin/sh

# SPDX-License-Identifier: MPL-2.0

rustc="$1"
shift
crate_name=
target=
previous=

for argument in "$@"; do
    if [ "$previous" = "--crate-name" ]; then
        crate_name="$argument"
    elif [ "$previous" = "--target" ]; then
        target="$argument"
    elif case "$argument" in --target=*) true;; *) false;; esac; then
        target=${argument#--target=}
    fi
    previous="$argument"
done

if [ "$crate_name" = "aster_fpu" ] && [ "$target" = "x86_64-unknown-none" ]; then
    exec "$rustc" "$@" -Ctarget-feature=-soft-float,+sse2,+ssse3,+sse4.1,+aes,+pclmulqdq -Awarnings
else
    exec "$rustc" "$@"
fi
