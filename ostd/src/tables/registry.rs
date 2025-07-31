//! A registry of tables by name.
//!
//! In most cases, you should use [`get_global_registry`] to get your registry since sharing tables anywhere in the
//! system is kind of the point.

// TODO: Arguably, this should use a table and call some service. However, having a clean interface is nice. It's easier
// to implement it manually for the moment, but in the future we may want to use some statically accessed tables and a
// service to allow observers to see tables being registered.

use alloc::{
    borrow::ToOwned,
    boxed::Box,
    format,
    string::String,
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    any::{type_name, Any},
    borrow::Borrow,
    fmt::Display,
    num::NonZeroUsize,
    str::FromStr,
    sync::atomic::{AtomicUsize, Ordering},
};

use hashbrown::HashMap;
use log::{error, info};
use spin::Once;

use crate::{
    sync::Mutex,
    tables::Table,
};

/// A path, or identifier, in a table registry.
///
/// Empty elements are allowed to reduce the number of error cases since they do not actually cause problems. However,
/// they are confusing and should be avoided.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Path(Vec<String>);
// TODO:OPTIMIZATION: Path should probably be a reversed linked list to make append and prefix fast

impl Display for Path {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0.join("."))
    }
}

impl FromStr for Path {
    type Err = core::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<_> = s.split('.').map(|s| s.to_owned()).collect();
        Ok(Path(parts))
    }
}

static PATH_UNIQUE_ID: AtomicUsize = AtomicUsize::new(0);

impl Path {
    /// An empty path.
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    /// Append a value to the path returning the new path. This is used inline to extend paths:
    /// `f(path.append("element")`.
    pub fn append(&self, e: impl Borrow<str>) -> Path {
        let mut path = self.0.clone();
        path.push(e.borrow().to_owned());
        Path(path)
    }

    /// Get the prefix of the path, aka the parent if you view a path as a hierarchy.
    pub fn prefix(&self) -> Path {
        let mut path = self.0.clone();
        path.pop();
        Path(path)
    }

    /// Append a unique identifier to produce a path which is unique and avoids any conflicts.
    pub fn append_unique(&self) -> Path {
        let mut path = self.0.clone();
        path.push(format!(
            "<{}>",
            PATH_UNIQUE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        Path(path)
    }
}

/// Create a [`Path`] from path syntax. This also supports interpolation.
///
/// For example,
///
/// ```
/// # use ostd::path;
/// let path = path!(system.subsystem.table);
/// ```
///
/// or
///
/// ```
/// # use ostd::path;
/// let x = "system";
/// let path = path!({x}.subsystem.table);
/// ```
///
/// this works with any type that implements `Display`.
///
/// You can also include `{?}` to generate a unique ID segment in the path: `path!(system.{?})`.
///
///
#[macro_export]
macro_rules! path {
    ($($segment:tt). *) => {{
        let path = $crate::tables::registry::Path::empty();
        $(let path = $crate::path_segment!(path, $segment);)*
        path
    }};
}
// TODO: Support subpaths interpolation.
// TODO: Make this const to allow const global paths.

/// **INTERNAL USE ONLY**
///
/// Convert a single path segment as passed to [`path!`].
#[macro_export]
macro_rules! path_segment {
    ($path:ident, {?}) => {
        $path.append_unique()
    };
    ($path:ident, $segment:ident) => {
        $path.append(stringify!($segment))
    };
    ($path:ident, {$segment:expr}) => {
        $path.append($segment.to_string())
    };
}

/// An opaque ID for a table. This provides a way to identify a table that is `Copy + Sync + Send` and hence can be
/// passed through tables with limitations on their message types. Generally [paths](Path) should be used instead since
/// they are easier to use during debugging.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TableId(NonZeroUsize);

/// A registry of tables by name. The type of the table element must be exactly the same when looking up a table, but
/// the table implementation need not be known during registration or lookup.
pub struct TableRegistry {
    inner: Mutex<TableRegistryInner>,
}

struct TableRegistryInner {
    // TODO: this needs to be garbage collected somehow.
    /// A map from ids to tables. The `dyn Any` value must be a `Weak<Table<T>>` for some `T`.
    map: HashMap<TableId, Box<dyn Any + Send + Sync + 'static>>,
    /// A map from paths to ID for performing path based lookup.
    id_to_path_map: HashMap<TableId, Path>,
    /// A map from IDs to paths; mostly for debugging.
    path_to_id_map: HashMap<Path, TableId>,
    /// The next ID to use for a path.
    next_id: NonZeroUsize,
}

// TODO: Use Result instead of Option.

impl TableRegistry {
    /// Create a new registry. This should rarely be used, since sharing a registry is the point, having a few widely
    /// shared registry is desireable. See [`get_global_registry`].
    fn new() -> Self {
        Self {
            inner: Mutex::new(TableRegistryInner {
                map: Default::default(),
                id_to_path_map: Default::default(),
                path_to_id_map: Default::default(),
                next_id: NonZeroUsize::new(1).unwrap(),
            }),
        }
    }

    /// Look up a table in the registry and return it if the contained type is the same as the registered type.
    pub fn lookup<T: 'static>(&self, path: &Path) -> Option<Arc<dyn Table<T>>> {
        let guard = self.inner.lock();
        let id = *guard.path_to_id_map.get(path)?;
        info!("Looking up table {path} {id:?}");
        Self::lookup_by_guard(&guard, id)
    }

    /// Look up a table by ID and return it if the contained type is the same as the registered type.
    pub fn lookup_by_id<T: 'static>(&self, id: TableId) -> Option<Arc<dyn Table<T>>> {
        let guard = self.inner.lock();
        Self::lookup_by_guard(&guard, id)
    }

    /// Look up a table by ID using the mutex guard.
    fn lookup_by_guard<T: 'static>(
        guard: &crate::sync::MutexGuard<TableRegistryInner>,
        id: TableId,
    ) -> Option<Arc<dyn Table<T>>> {
        let any = guard.map.get(&id)?;
        // TODO: Return Error instead of Option.
        let Some(weak) = any.downcast_ref::<Weak<dyn Table<T>>>() else {
            error!(
                "Table {id:?} type is incorrect, requested {}, actual {:#?}",
                type_name::<T>(),
                any.as_ref().type_id()
            );
            return None;
        };
        let Some(strong) = weak.upgrade() else {
            error!("Table {id:?} type {}, was deallocated", type_name::<T>(),);
            return None;
        };
        Some(strong)
    }

    /// Look up the ID for a path.
    pub fn lookup_id(&self, path: &Path) -> Option<TableId> {
        self.inner.lock().path_to_id_map.get(path).cloned()
    }

    /// Look up the path for a table ID.
    pub fn lookup_path(&self, id: TableId) -> Option<Path> {
        self.inner.lock().id_to_path_map.get(&id).cloned()
    }

    // TODO: Tables are never removed. We should have a cleaning pass every so often to remove dead references.

    /// Add a table to the registry.
    pub fn register<T: 'static>(&self, path: Path, table: Arc<dyn Table<T>>) -> TableId {
        let mut inner = self.inner.lock();
        let id = TableId(inner.next_id);
        inner.next_id = inner
            .next_id
            .checked_add(1)
            .expect("there were more than 2^64-1 tables created");
        inner.id_to_path_map.insert(id, path.clone());
        inner.path_to_id_map.insert(path.clone(), id);
        let erased_table = Box::new(Arc::downgrade(&table));
        info!(
            "Registering table {path} (assigned id {id:?}) with type {} ({:#?})",
            type_name::<T>(),
            erased_table.as_ref().type_id()
        );
        inner.map.insert(id, erased_table);
        id
    }

    /// Lookup a table or use the provided closure to create and register it.
    /// 
    /// This will return None if there is a registered table, but it is the wrong time.
    pub fn lookup_or_register<T: 'static>(
        &self,
        path: Path,
        table_fn: impl FnOnce() -> Arc<dyn Table<T>>,
    ) -> Option<Arc<dyn Table<T>>> {
        let key_exists = {
            let guard = self.inner.lock();
            guard.path_to_id_map.contains_key(&path)
        };
        if key_exists {
            self.lookup::<T>(&path)
        } else {
            let t = table_fn();
            self.register(path, t.clone());
            Some(t)
        }
    }
}

static TABLE_REGISTRY: Once<TableRegistry> = Once::new();

/// Get the global registry which can be used from any part of the system. This is generally the register you want to
/// use.
pub fn get_global_table_registry() -> &'static TableRegistry {
    TABLE_REGISTRY.call_once(TableRegistry::new)
}

#[cfg(ktest)]
mod test {
    use alloc::{string::ToString as _, vec};

    use super::*;
    use crate::prelude::ktest;

    #[ktest]
    fn test_path_creation() {
        let path = Path(vec![
            "src".to_string(),
            "main".to_string(),
            "rs".to_string(),
        ]);
        assert_eq!(path.to_string(), "src.main.rs");

        let empty_path = Path::empty();
        assert_eq!(empty_path.to_string(), "");
    }

    #[ktest]
    fn test_path_append() {
        let base_path = Path(vec!["src".to_string()]);
        let new_path = base_path.append("main");
        assert_eq!(new_path.to_string(), "src.main");

        let empty_path = Path::empty();
        let appended_path = empty_path.append("element");
        assert_eq!(appended_path.to_string(), "element");
    }

    #[ktest]
    fn test_path_prefix() {
        let path = Path(vec!["src".to_string(), "main".to_string()]);
        let prefix = path.prefix();
        assert_eq!(prefix.to_string(), "src");

        let empty_path = Path::empty();
        let prefix_empty = empty_path.prefix();
        assert_eq!(prefix_empty.to_string(), "");
    }

    #[ktest]
    fn test_path_append_unique() {
        let base_path = Path(vec!["src".to_string()]);
        let unique_path = base_path.append_unique();
        assert!(unique_path.to_string().starts_with("src.<"));
    }

    #[ktest]
    fn test_path_macro() {
        let path = path!(system.subsystem.table);
        assert_eq!(path.to_string(), "system.subsystem.table");
    }

    #[ktest]
    fn test_path_macro_interpolate_string() {
        let x = "system";
        let path = path!({ x }.subsystem.table);
        assert_eq!(path.to_string(), "system.subsystem.table");
    }
    #[ktest]
    fn test_path_macro_interpolate_int() {
        let x = 3;
        let path = path!(system.{ x }.subsystem.table);
        assert_eq!(path.to_string(), "system.3.subsystem.table");
    }
}
