// SPDX-License-Identifier: MPL-2.0

//! Logging command-line parameters.
//! 
//! This is on top of the `loglevel` parameter handled by the `cmdline::early`.

use alloc::string::{String, ToString as _};

use aster_cmdline::parse::{ParamError, ParseParamValue};
use component::{ComponentInitError, init_component};
use hashbrown::HashMap;
use ostd::log::{Level, LevelFilter};
use spin::Once;

/// Per-prefix log level overrides, parsed from `prefix_loglevel=prefix:level,...`.
///
/// Enables a specific log level for specific `__log_prefix!()` prefixes,
/// independent of the global level set via `loglevel`.
pub struct PrefixLevelFilter(HashMap<String, LevelFilter>);

impl PrefixLevelFilter {
    /// Returns `true` if `level` passes the filter configured for `prefix`.
    pub fn is_enabled(&self, prefix: &str, level: Level) -> bool {
        // Log prefixes has a trailing ": ".
        let key = prefix.strip_suffix(": ").unwrap_or(prefix);
        self.0
            .get(key)
            .is_some_and(|filter| filter.is_enabled(level))
    }
}

/// Value format: `prefix:level[,prefix:level...]`, e.g. `iommu:debug,virtio:warn`. `level` accepts
/// the same numeric (0..=8) or textual forms as `loglevel`. An empty prefix is allowed and will
/// select logging without a prefix.
impl ParseParamValue for PrefixLevelFilter {
    fn parse_param(value: &str) -> Result<Self, ParamError> {
        if value.is_empty() {
            return Err(ParamError::InvalidValue);
        }

        let mut map = HashMap::new();
        for entry in value.split(',') {
            let (prefix, level) = entry.split_once(':').ok_or(ParamError::InvalidValue)?;
            let level = aster_cmdline::parse_utils::parse_loglevel_at(level.as_bytes())
                .ok_or(ParamError::InvalidValue)?;
            map.insert(prefix.to_string(), level);
        }

        Ok(PrefixLevelFilter(map))
    }
}

/// Per-prefix log level overrides, set from the `prefix_loglevel` kernel command-line parameter.
static PREFIX_LOGLEVEL: Once<PrefixLevelFilter> = Once::new();

// Enables logging at a specific level for specific `__log_prefix!()` prefixes, independent of (and
// in addition to) the global `loglevel`.
//
// Value format: `prefix:level[,prefix:level...]`, e.g. `iommu:debug,virtio:warn`. An empty prefix is allowed and will
// select logging without a prefix.
aster_cmdline::define_kv_param!("prefix_loglevel", PREFIX_LOGLEVEL);

/// The per-prefix log level checker registered with `ostd::log::inject_prefix_filter`.
fn check_prefix_level(prefix: &str, level: Level) -> bool {
    match PREFIX_LOGLEVEL.get() {
        Some(filter) => filter.is_enabled(prefix, level),
        None => false,
    }
}

#[init_component]
fn init() -> Result<(), ComponentInitError> {
    ostd::log::inject_prefix_filter(check_prefix_level);
    Ok(())
}

#[cfg(ktest)]
mod test {
    use ostd::prelude::*;

    use super::*;

    #[ktest]
    fn prefix_level_filter_parse_ok() {
        let filter = PrefixLevelFilter::parse_param("iommu:debug").unwrap();
        assert!(filter.is_enabled("iommu: ", Level::Debug));
        assert!(!filter.is_enabled("virtio: ", Level::Debug));

        let filter = PrefixLevelFilter::parse_param("iommu:debug,virtio:warn").unwrap();
        assert!(filter.is_enabled("iommu: ", Level::Debug));
        assert!(filter.is_enabled("virtio: ", Level::Warning));
        assert!(!filter.is_enabled("virtio: ", Level::Info));

        let filter = PrefixLevelFilter::parse_param("iommu:7").unwrap();
        assert!(filter.is_enabled("iommu: ", Level::Debug));
        
        let filter = PrefixLevelFilter::parse_param(" :7").unwrap();
        assert!(filter.is_enabled("", Level::Debug));
    }

    #[ktest]
    fn prefix_level_filter_parse_err() {
        for value in [
            "",
            "iommu",
            ":debug",
            "iommu:",
            "iommu:bogus",
            "iommu:9",
            "iommu:debug,",
        ] {
            assert!(PrefixLevelFilter::parse_param(value).is_err());
        }
    }
}
