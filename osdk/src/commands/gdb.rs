// SPDX-License-Identifier: MPL-2.0

use crate::commands::util::is_tdx_enabled;
use crate::{config::scheme::Action, error::Errno, error_msg, warn_msg};
use std::process::exit;

/// Adapt a `Config` to enable GDB server based on the provided args. Returns an optional
/// `VscLaunchConfig` which ensures VSCode launch file cleanup when it is dropped.
pub fn adapt_for_gdb_server(
    config: &mut Action,
    gdb_server_str: &str,
) -> Option<vsc::VscLaunchConfig> {
    let gdb_server_args = GdbServerArgs::from_str(gdb_server_str);

    // Add GDB server arguments to QEMU.
    let qemu_gdb_args = {
        let gdb_stub_addr = gdb_server_args.host_addr.as_str();
        match stub_type_of(gdb_stub_addr) {
            StubAddrType::Unix => {
                format!(
                    " -chardev socket,path={},server=on,wait=off,id=gdb0 -gdb chardev:gdb0",
                    gdb_stub_addr
                )
            }
            StubAddrType::Tcp => {
                format!(
                    " -gdb tcp:{}",
                    tcp_addr_util::format_tcp_addr(gdb_stub_addr)
                )
            }
        }
    };
    config.qemu.args += &qemu_gdb_args;

    if gdb_server_args.wait_client {
        config.qemu.args += " -S";
    }

    if is_tdx_enabled() {
        let target = "-object tdx-guest,";
        if let Some(pos) = config.qemu.args.find(target) {
            let insert_pos = pos + target.len();
            config.qemu.args.insert_str(insert_pos, "debug=on,");
        } else {
            warn_msg!(
                "TDX is enabled, but the TDX guest object is not found in the QEMU arguments"
            );
        }
    }

    // Ensure debug info added when debugging in the release profile.
    if config.build.profile.contains("release") {
        config
            .build
            .override_configs
            .push(format!("profile.{}.debug=true", config.build.profile));
    }

    gdb_server_args.vsc_launch_file.then(|| {
        vsc::check_gdb_config(&gdb_server_args);
        let profile = super::util::profile_name_adapter(&config.build.profile);
        vsc::VscLaunchConfig::new(profile, &gdb_server_args.host_addr)
    })
}

/// Parsed gdb server args
pub struct GdbServerArgs {
    pub host_addr: String,
    pub wait_client: bool,
    pub vsc_launch_file: bool,
}

impl GdbServerArgs {
    pub fn from_str(args: &str) -> Self {
        let mut host_addr = ".osdk-gdb-socket".to_string();
        let mut wait_client = false;
        let mut vsc_launch_file = false;

        for arg in args.split(",") {
            let kv = arg.split('=').collect::<Vec<_>>();
            match kv.as_slice() {
                ["addr", addr] => host_addr = addr.to_string(),
                ["wait-client"] => wait_client = true,
                ["vscode"] => vsc_launch_file = true,
                _ => {
                    error_msg!("Invalid GDB server argument: {}", arg);
                    exit(Errno::Cli as _);
                }
            }
        }

        GdbServerArgs {
            host_addr,
            wait_client,
            vsc_launch_file,
        }
    }
}

/// Utilities for determining stub address type and tcp formatting
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StubAddrType {
    Unix, // Unix Domain Socket
    Tcp,  // IP_ADDR:PORT
}

fn stub_type_of(stub: &str) -> StubAddrType {
    if stub.split(':').next_back().unwrap().parse::<u16>().is_ok() {
        return StubAddrType::Tcp;
    }
    StubAddrType::Unix
}

mod tcp_addr_util {
    use crate::{error::Errno, error_msg};
    use std::process::exit;

    fn strip_tcp_prefix(addr: &str) -> &str {
        addr.strip_prefix("tcp:").unwrap_or(addr)
    }

    fn parse_tcp_addr(addr: &str) -> (&str, u16) {
        let addr = strip_tcp_prefix(addr);
        if !addr.contains(':') {
            error_msg!("Ambiguous GDB server address, use '[IP]:PORT' format");
            exit(Errno::ParseMetadata as _);
        }
        let mut iter = addr.split(':');
        let host = iter.next().unwrap();
        let port = iter.next().unwrap().parse().unwrap();
        (host, port)
    }

    pub fn format_tcp_addr(tcp_addr: &str) -> String {
        let (host, port) = parse_tcp_addr(tcp_addr);
        format!("{}:{}", host, port)
    }
}

/// VS Code launch file generator for GDB
pub mod vsc {
    use crate::{
        commands::util::bin_file_name,
        util::{get_cargo_metadata, get_kernel_crate},
    };
    use serde_json::{Value, from_str};
    use std::{
        fs::{read_to_string, write as write_file},
        path::Path,
    };

    use super::{GdbServerArgs, StubAddrType, stub_type_of, tcp_addr_util};

    const VSC_DIR: &str = ".vscode";

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct Existence {
        vsc_dir: bool,
        launch_file: bool,
    }

    fn workspace_root() -> String {
        get_cargo_metadata(None::<&str>, None::<&[&str]>).unwrap()["workspace_root"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    #[derive(Debug, Default)]
    pub struct VscLaunchConfig {
        existence: Existence,
        backup_launch_path: Option<String>,
    }

    impl VscLaunchConfig {
        pub fn new(profile: &str, addr: &str) -> Self {
            let workspace = workspace_root();
            let workspace = Path::new(&workspace);
            let launch_file_path = workspace.join(VSC_DIR).join("launch.json");
            let existence = Existence {
                vsc_dir: workspace.join(VSC_DIR).exists(),
                launch_file: launch_file_path.exists(),
            };

            let backup_launch_path = existence.launch_file.then(|| {
                let backup_launch_path = launch_file_path.with_extension("bak");
                std::fs::copy(&launch_file_path, &backup_launch_path).unwrap();
                backup_launch_path.to_string_lossy().to_string()
            });

            if !existence.vsc_dir {
                std::fs::create_dir(workspace.join(VSC_DIR)).unwrap();
            }
            generate_vsc_launch_file(workspace, profile, addr).unwrap();

            VscLaunchConfig {
                existence,
                backup_launch_path,
            }
        }
    }

    impl Drop for VscLaunchConfig {
        fn drop(&mut self) {
            // remove generated files
            if !self.existence.vsc_dir {
                std::fs::remove_dir_all(Path::new(&workspace_root()).join(VSC_DIR)).unwrap();
                return;
            }
            if !self.existence.launch_file {
                std::fs::remove_file(
                    Path::new(&workspace_root())
                        .join(VSC_DIR)
                        .join("launch.json"),
                )
                .unwrap();
                return;
            }
            // restore backup launch file
            if let Some(backup_launch_path) = &self.backup_launch_path {
                std::fs::copy(
                    backup_launch_path,
                    Path::new(&workspace_root())
                        .join(VSC_DIR)
                        .join("launch.json"),
                )
                .unwrap();
                std::fs::remove_file(backup_launch_path).unwrap();
            }
        }
    }

    /// Exit if the QEMU GDB server configuration is not valid
    pub fn check_gdb_config(args: &GdbServerArgs) {
        use crate::{error::Errno, error_msg};
        use std::process::exit;

        // check GDB server address
        let gdb_stub_addr = args.host_addr.as_str();
        if gdb_stub_addr.is_empty() {
            error_msg!("GDB server address is required to generate a VSCode launch file");
            exit(Errno::ParseMetadata as _);
        }
        if stub_type_of(gdb_stub_addr) != StubAddrType::Tcp {
            error_msg!(
                "Non-TCP GDB server address is not supported under '--gdb-server vscode' currently"
            );
            exit(Errno::ParseMetadata as _);
        }
    }

    fn generate_vsc_launch_file(
        workspace: impl AsRef<Path>,
        profile: &str,
        addr: &str,
    ) -> Result<(), std::io::Error> {
        let contents = include_str!("launch.json.template")
            .replace("#PROFILE#", profile)
            .replace("#CRATE_NAME#", &get_kernel_crate().name)
            .replace("#BIN_NAME#", &bin_file_name())
            .replace(
                "#ADDR_PORT#",
                tcp_addr_util::format_tcp_addr(addr).trim_start_matches(':'),
            );

        let original_items: Option<Value> = {
            let launch_file_path = workspace.as_ref().join(VSC_DIR).join("launch.json");
            let src_path = if launch_file_path.exists() {
                launch_file_path
            } else {
                launch_file_path.with_extension("bak")
            };
            src_path
                .exists()
                .then(|| from_str(&read_to_string(&src_path).unwrap()).unwrap())
        };

        let contents = if let Some(mut original_items) = original_items {
            let items: Value = from_str(&contents)?;
            for item in items["configurations"].as_array().unwrap() {
                if original_items["configurations"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|x| x["name"].as_str() == item["name"].as_str())
                {
                    println!("[{}/launch.json]{} already exists", VSC_DIR, item["name"]);
                    // no override for configurations with the same name
                    continue;
                }
                original_items["configurations"]
                    .as_array_mut()
                    .unwrap()
                    .push(item.clone());
            }
            serde_json::to_string_pretty(&original_items).unwrap()
        } else {
            contents
        };

        let launch_file_path = workspace.as_ref().join(VSC_DIR).join("launch.json");
        write_file(launch_file_path, contents)
    }
}
