// SPDX-License-Identifier: MPL-2.0

pub(crate) struct ProcfsConfig {
    pub buffer_bytes: usize,
}

impl ProcfsConfig {
    pub fn from_kcmdline() -> Self {
        let buffer_bytes = ostd::boot::boot_info()
            .kernel_cmdline
            .split_whitespace()
            .filter_map(|arg| arg.strip_prefix("oqueue_procfs.buffer_bytes="))
            .filter_map(|value| value.parse().ok())
            .next_back()
            .unwrap_or(1_048_576);

        Self { buffer_bytes }
    }
}
