//! Provide safe access to the kernel command-line.

use alloc::vec::Vec;

use spin::Once;

use crate::boot::EARLY_INFO;

struct KernelCmdLine {
    args: Vec<(&'static str, &'static str)>,
}

impl KernelCmdLine {
    fn new(kcmdline: &'static str) -> Self {
        let mut args = Vec::new();
        let mut iter = kcmdline.split_whitespace();
        while let Some(arg) = iter.next() {
            if let Some((key, value)) = arg.split_once('=') {
                args.push((key, value));
            }
        }

        Self { args }
    }

    fn get_value(&self, key: &str) -> Option<&'static str> {
        self.args
            .iter()
            .find_map(|&(k, v)| if k == key { Some(v) } else { None })
    }
}

static KERNEL_CMDLINE: Once<KernelCmdLine> = Once::new();

fn init_kernel_cmdline() {
    KERNEL_CMDLINE.call_once(|| {
        let kcmdline = EARLY_INFO.get().unwrap().kernel_cmdline;
        KernelCmdLine::new(kcmdline)
    });
}

/// Get the value of a kernel command-line argument. This will find arguments in the form
/// `key=value`.
pub fn get_kcmdline_arg(key: &str) -> Option<&'static str> {
    init_kernel_cmdline();
    KERNEL_CMDLINE.get().unwrap().get_value(key)
}
