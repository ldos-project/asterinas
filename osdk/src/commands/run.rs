// SPDX-License-Identifier: MPL-2.0

use super::{build::create_base_and_cached_build, util::DEFAULT_TARGET_RELPATH};
use crate::{
    config::{Config, scheme::ActionChoice},
    util::{get_kernel_crate, get_target_directory},
};

pub fn execute_run_command(config: &Config, gdb_server_args: Option<&str>) {
    let cargo_target_directory = get_target_directory();
    let osdk_output_directory = cargo_target_directory.join(DEFAULT_TARGET_RELPATH);

    let target_info = get_kernel_crate();

    let mut config = config.clone();

    let _vsc_launch_file = if let Some(gdb_server_str) = gdb_server_args {
        super::gdb::adapt_for_gdb_server(&mut config.run, gdb_server_str)
    } else {
        None
    };

    let default_bundle_directory = osdk_output_directory.join(&target_info.name);
    let bundle = create_base_and_cached_build(
        target_info,
        default_bundle_directory,
        &osdk_output_directory,
        &cargo_target_directory,
        &config,
        ActionChoice::Run,
        &[],
    );

    bundle.run(&config, ActionChoice::Run);
}
