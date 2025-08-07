// SPDX-License-Identifier: MPL-2.0

#![expect(unused_variables)]

//! The module to parse kernel command-line arguments.
//!
//! The format of the Asterinas command line string conforms
//! to the Linux kernel command line rules:
//!
//! <https://www.kernel.org/doc/html/v6.4/admin-guide/kernel-parameters.html>
//!

use alloc::{
    collections::BTreeMap,
    ffi::CString,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use core::{any::type_name, str::FromStr};

use log::error;
use spin::Once;

#[derive(PartialEq, Debug, Clone)]
struct InitprocArgs {
    path: Option<String>,
    argv: Vec<CString>,
    envp: Vec<CString>,
}

/// Kernel module arguments
#[derive(PartialEq, Debug, Clone)]
pub enum ModuleArg {
    /// A string argument
    Arg(CString),
    /// A key-value argument
    KeyVal(CString, CString),
}

/// The struct to store the parsed kernel command-line arguments.
#[derive(Debug, Clone)]
pub struct KCmdlineArg {
    initproc: InitprocArgs,
    module_args: BTreeMap<String, Vec<ModuleArg>>,
}

impl KCmdlineArg {
    /// Gets the path of the initprocess.
    pub fn get_initproc_path(&self) -> Option<&str> {
        self.initproc.path.as_deref()
    }
    /// Gets the argument vector(argv) of the initprocess.
    pub fn get_initproc_argv(&self) -> &Vec<CString> {
        &self.initproc.argv
    }
    /// Gets the environment vector(envp) of the initprocess.
    pub fn get_initproc_envp(&self) -> &Vec<CString> {
        &self.initproc.envp
    }
    /// Gets the argument vector of a kernel module.
    pub fn get_module_args(&self, module: &str) -> Option<&Vec<ModuleArg>> {
        self.module_args.get(module)
    }

    /// Return the value of of an argument for a specific module.
    ///
    /// This returns `None` if the argument value isn't available for any reason. If the argument is missing this is
    /// considered a "normal" missing argument. If the argument is:
    ///
    /// * provided as a flag (without a value),
    /// * does not convert successfully to UTF8,
    /// * does not parse into type T.
    ///
    /// this prints an error and returns `None`. If there is more than once instance of the argument the value with the
    /// last one will be returned. This is a bit crude as error handling, but it provides a concise and readable way to
    /// access parameters. If code needs to handle things in a more careful way you can write it against
    /// [`Self::get_module_args`] instead.
    ///
    /// In generally, this should only be used for testing or benchmarking since it is not exposed to users very well. The
    /// exception would be for very low level configuration where runtime configuration is impossible.
    pub fn get_module_arg_by_name<T: FromStr>(&self, module: &str, arg_name: &str) -> Option<T> {
        if let Some(module_args) = self.get_module_args(module) {
            let mut vals: Vec<_> = module_args.iter().filter_map(|arg| {
                match arg {
                    ModuleArg::Arg(name) if name.as_bytes() == arg_name.as_bytes() => {
                        error!("Argument {arg_name} (expected type {}) was provided without an value. Ignored.", type_name::<T>());
                        None
                    },
                    ModuleArg::KeyVal(name, val) if name.as_bytes() == arg_name.as_bytes() => {
                        match String::from_utf8(val.as_bytes().to_vec()).map(|s| T::from_str(&s)) {
                            Ok(Ok(v)) => Some(v),
                            Ok(Err(e)) => {
                                error!("Argument {arg_name} (expected type {}) with value {:?} could not be parsed. Ignored.", type_name::<T>(), val);
                                None
                            },
                            Err(e) => {
                                // This should be unreachable given that a version to and from str has already happened, but no reason to crash.
                                error!("Argument {arg_name} with value {:?} could not be converted into str with error {}. Ignored.", val, e);
                                None
                            }

                        }
                    }
                    _ => None,
                }
            }).collect();
            if vals.len() > 1 {
                error!("Argument {arg_name} provided more than once. Discarding all but last.");
            }
            vals.pop()
        } else {
            None
        }
    }

    /// Returns whether a specific flag (argument without a value) is set for a given module.
    ///
    /// This returns `false` if the argument value isn't provided. If the argument provided with a value this prints an
    /// error and returns `true`. This is a bit crude as error handling, but it provides a concise and readable way to
    /// access parameters. If code needs to handle things in a more careful way you can write it against
    /// [`Self::get_module_args`] instead.
    ///
    /// In generally, this should only be used for testing or benchmarking since it is not exposed to users very well. The
    /// exception would be for very low level configuration where runtime configuration is impossible.
    pub fn get_module_flag_by_name(&self, module: &str, arg_name: &str) -> bool {
        if let Some(module_args) = self.get_module_args(module) {
            module_args.iter().any(|arg| match arg {
                ModuleArg::Arg(name) if name.as_bytes() == arg_name.as_bytes() => true,
                ModuleArg::KeyVal(name, val) if name.as_bytes() == arg_name.as_bytes() => {
                    error!(
                        "Flag {arg_name} was provided with a value {:?}. Ignored.",
                        val
                    );
                    true
                }
                _ => false,
            })
        } else {
            false
        }
    }
}

/// Splits the command line string by spaces but preserve ones that are protected by double quotes(`"`).
fn split_arg(input: &str) -> impl Iterator<Item = &str> {
    let mut inside_quotes = false;

    input.split(move |c: char| {
        if c == '"' {
            inside_quotes = !inside_quotes;
        }

        !inside_quotes && c.is_whitespace()
    })
}

/// Define the way to parse a string to `KCmdlineArg`.
impl From<&str> for KCmdlineArg {
    fn from(cmdline: &str) -> Self {
        // What we construct.
        let mut result: KCmdlineArg = KCmdlineArg {
            initproc: InitprocArgs {
                path: None,
                argv: Vec::new(),
                envp: Vec::new(),
            },
            module_args: BTreeMap::new(),
        };

        // Every thing after the "--" mark is the initproc arguments.
        let mut kcmdline_end = false;

        // The main parse loop. The processing steps are arranged (not very strictly)
        // by the analysis over the Backus–Naur form syntax tree.
        for arg in split_arg(cmdline) {
            // Cmdline => KernelArg "--" InitArg
            // KernelArg => Arg "\s+" KernelArg | %empty
            // InitArg => Arg "\s+" InitArg | %empty
            if kcmdline_end {
                if result.initproc.path.is_none() {
                    panic!("Initproc arguments provided but no initproc path specified!");
                }
                result.initproc.argv.push(CString::new(arg).unwrap());
                continue;
            }
            if arg == "--" {
                kcmdline_end = true;
                continue;
            }
            // Arg => Entry | Entry "=" Value
            let arg_pattern: Vec<_> = arg.split('=').collect();
            let (entry, value) = match arg_pattern.len() {
                1 => (arg_pattern[0], None),
                2 => (arg_pattern[0], Some(arg_pattern[1])),
                _ => {
                    log::warn!(
                        "[KCmdline] Unable to parse kernel argument {}, skip for now",
                        arg
                    );
                    continue;
                }
            };
            // Entry => Module "." ModuleOptionName | KernelOptionName
            let entry_pattern: Vec<_> = entry.split('.').collect();
            let (node, option) = match entry_pattern.len() {
                1 => (None, entry_pattern[0]),
                2 => (Some(entry_pattern[0]), entry_pattern[1]),
                _ => {
                    log::warn!(
                        "[KCmdline] Unable to parse entry {} in argument {}, skip for now",
                        entry,
                        arg
                    );
                    continue;
                }
            };
            if let Some(modname) = node {
                let modarg = if let Some(v) = value {
                    ModuleArg::KeyVal(
                        CString::new(option.to_string()).unwrap(),
                        CString::new(v).unwrap(),
                    )
                } else {
                    ModuleArg::Arg(CString::new(option).unwrap())
                };
                result
                    .module_args
                    .entry(modname.to_string())
                    .and_modify(|v| v.push(modarg.clone()))
                    .or_insert(vec![modarg.clone()]);
                continue;
            }
            // KernelOptionName => /*literal string alternatives*/ | /*init environment*/
            if let Some(value) = value {
                // The option has a value.
                match option {
                    "init" => {
                        if let Some(v) = &result.initproc.path {
                            panic!("Initproc assigned twice in the command line!");
                        }
                        result.initproc.path = Some(value.to_string());
                    }
                    _ => {
                        // If the option is not recognized, it is passed to the initproc.
                        // Pattern 'option=value' is treated as the init environment.
                        let envp_entry = CString::new(option.to_string() + "=" + value).unwrap();
                        result.initproc.envp.push(envp_entry);
                    }
                }
            } else {
                // There is no value, the entry is only a option.

                // If the option is not recognized, it is passed to the initproc.
                // Pattern 'option' without value is treated as the init argument.
                let argv_entry = CString::new(option.to_string()).unwrap();
                result.initproc.argv.push(argv_entry);
            }
        }

        result
    }
}

static KERNEL_CMD_LINE: Once<KCmdlineArg> = Once::new();

// Set the global kernel command line. This call will be ignored if the command line has already been set.
pub(crate) fn set_kernel_cmd_line(cmd_line: KCmdlineArg) {
    if let Some(first) = KERNEL_CMD_LINE.get() {
        error!("Kernel command line was set more than once. The first was: {first:?}\nThe second was: {cmd_line:?}");
    }
    KERNEL_CMD_LINE.call_once(|| cmd_line);
}

/// Get the global kernel command line. For simple configuration options use this along with
/// [`KCmdlineArg::get_module_arg_by_name`] and  [`KCmdlineArg::get_module_flag_by_name`]. A reasonable pattern is:
///
/// ```no_run
/// get_kernel_cmd_line().and_then(|cl| cl.get_module_flag_by_name("mod", "flag"))
/// ```
///
/// In generally, this should only be used for testing or benchmarking since it is not exposed to users very well. The
/// exception would be for very low level configuration where runtime configuration is impossible.
pub fn get_kernel_cmd_line() -> Option<&'static KCmdlineArg> {
    KERNEL_CMD_LINE.get()
}

#[cfg(ktest)]
mod test {
    use ostd::prelude::ktest;

    use super::*;

    #[ktest]
    fn test_get_module_arg_by_name_with_value() {
        let cmdline = "module.arg=value";
        let kcmdline = KCmdlineArg::from(cmdline);

        assert_eq!(
            kcmdline.get_module_arg_by_name::<String>("module", "arg"),
            Some("value".to_string())
        );
    }

    #[ktest]
    fn test_get_module_arg_by_name_second_with_value() {
        let cmdline = "module.other module.arg=value";
        let kcmdline = KCmdlineArg::from(cmdline);

        assert_eq!(
            kcmdline.get_module_arg_by_name::<String>("module", "arg"),
            Some("value".to_string())
        );
    }

    #[ktest]
    fn test_get_module_arg_by_name_with_multiple_values() {
        let cmdline = "module.arg=value module.arg=value2";
        let kcmdline = KCmdlineArg::from(cmdline);

        assert_eq!(
            kcmdline.get_module_arg_by_name::<String>("module", "arg"),
            Some("value2".to_string())
        );
    }

    #[ktest]
    fn test_get_module_arg_by_name_with_value_to_type() {
        let cmdline = "module.arg=42";
        let kcmdline = KCmdlineArg::from(cmdline);

        assert_eq!(
            kcmdline.get_module_arg_by_name::<i32>("module", "arg"),
            Some(42)
        );
    }

    #[ktest]
    fn test_get_module_arg_by_name_with_invalid_value() {
        let cmdline = "module.arg=notanint";
        let kcmdline = KCmdlineArg::from(cmdline);

        assert_eq!(
            kcmdline.get_module_arg_by_name::<i32>("module", "arg"),
            None
        );
    }

    #[ktest]
    fn test_get_module_arg_by_name_missing() {
        let cmdline = "module.arg=value";
        let kcmdline = KCmdlineArg::from(cmdline);

        assert_eq!(
            kcmdline.get_module_arg_by_name::<String>("module", "missing"),
            None
        );
    }

    #[ktest]
    fn test_get_module_flag_by_name() {
        let cmdline = "module.arg";
        let kcmdline = KCmdlineArg::from(cmdline);

        assert!(kcmdline.get_module_flag_by_name("module", "arg"));
    }

    #[ktest]
    fn test_get_module_flag_by_name_second() {
        let cmdline = "module.other module.arg";
        let kcmdline = KCmdlineArg::from(cmdline);

        assert!(kcmdline.get_module_flag_by_name("module", "arg"));
    }

    #[ktest]
    fn test_get_module_flag_by_name_missing() {
        let cmdline = "module.arg";
        let kcmdline = KCmdlineArg::from(cmdline);

        assert!(!kcmdline.get_module_flag_by_name("module", "missing"));
    }

    #[ktest]
    fn test_get_module_flag_by_name_with_value() {
        let cmdline = "module.arg=value";
        let kcmdline = KCmdlineArg::from(cmdline);

        assert!(kcmdline.get_module_flag_by_name("module", "arg"));
    }
}
