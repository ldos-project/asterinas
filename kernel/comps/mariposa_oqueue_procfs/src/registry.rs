// SPDX-License-Identifier: MPL-2.0

use alloc::{format, string::String, sync::Arc};
use core::fmt::Write;

use hashbrown::HashMap;
use ostd::{
    orpc::{
        oqueue::{OQueueBase as _, ObservationQuery, registry::lookup_by_path},
        path::{Path, PathComponentRef},
    },
    sync::Mutex,
};
use serde::Serialize;
use snafu::{OptionExt, ResultExt};

use crate::{ProcfsError, stream::StreamSource};

type StreamFactory =
    Arc<dyn Fn() -> Result<Arc<dyn StreamSource>, ProcfsError> + Send + Sync + 'static>;

static REGISTRY: Mutex<Option<HashMap<String, StreamFactory>>> = Mutex::new(None);

pub fn register_oqueue<T, U>(path: Path, projection: fn(&T) -> U)
where
    T: Send + 'static,
    U: Copy + Send + Serialize + 'static,
{
    let proc_path = map_path(&path);
    let path_for_factory = path.clone();
    let factory: StreamFactory = Arc::new(move || {
        let oqueue =
            lookup_by_path::<T>(&path_for_factory).context(crate::error::QueueGoneSnafu)?;
        let observer = oqueue
            .attach_strong_observer(ObservationQuery::new(projection))
            .context(crate::error::ObserveFailedSnafu)?;
        Ok(crate::stream::TypedStreamSource::<U>::new(
            path_for_factory.clone(),
            core::any::type_name::<U>(),
            observer,
        ))
    });

    let mut guard = REGISTRY.lock();
    guard
        .get_or_insert_with(HashMap::new)
        .insert(proc_path, factory);
}

pub fn open_stream(proc_path: &str) -> Result<Arc<dyn StreamSource>, ProcfsError> {
    let factory = {
        let guard = REGISTRY.lock();
        guard
            .as_ref()
            .and_then(|m| m.get(proc_path))
            .cloned()
            .context(crate::error::UnknownPathSnafu)?
    };

    factory()
}

pub fn paths() -> alloc::vec::Vec<String> {
    REGISTRY
        .lock()
        .as_ref()
        .map_or_else(Default::default, |m| m.keys().cloned().collect())
}

pub fn has_registered_prefix(prefix: &str) -> bool {
    let normalized_prefix = prefix.trim_end_matches('/');
    let with_slash = format!("{}/", normalized_prefix);

    REGISTRY.lock().as_ref().is_some_and(|m| {
        m.keys()
            .any(|path| path == normalized_prefix || path.starts_with(&with_slash))
    })
}

pub fn map_path(path: &Path) -> String {
    let mut out = String::from("/proc/oqueues");
    for component in path.components() {
        out.push('/');
        match component.borrow() {
            PathComponentRef::Name(name) => out.push_str(name),
            PathComponentRef::Index(index) => {
                write!(&mut out, "{}", index).unwrap();
            }
        }
    }
    out
}

#[cfg(ktest)]
mod test {
    use ostd::{path, prelude::*};

    use super::*;

    #[ktest]
    fn maps_named_path() {
        assert_eq!(
            map_path(&path!(scheduler.events)),
            "/proc/oqueues/scheduler/events"
        );
    }

    #[ktest]
    fn maps_index_path() {
        assert_eq!(map_path(&path!(raid1.io[0])), "/proc/oqueues/raid1/io/0");
    }
}
