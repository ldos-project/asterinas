# SPDX-License-Identifier: MPL-2.0

watchexec -w $1 -r -c -- "make build && make format && make check"
