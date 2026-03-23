// SPDX-License-Identifier: MPL-2.0

//! Paths for identifying servers and OQueues and patterns for matching against paths.
//!
//! Paths are a sequence of components. Each component is a name or an index (number). They are
//! written in the form: `name.name[index]`. This syntax requires that every index must come
//! immediately after a name. This not required by the [`Path`] and [`PathPattern`] types, but is
//! enforced in the syntax.

use alloc::{borrow::ToOwned, string::String, vec, vec::Vec};
use core::{fmt::Display, hash::Hash};

// TODO(arthurp): PERFORMANCE: Paths are constructed inefficiently with a lot of potential
// allocations.

/// A reference to `PathComponent` which ignores whether the name is owned. This is used for pattern
/// matching on [`PathComponent`].
#[derive(PartialEq, Eq, Hash)]
pub enum PathComponentRef<'a> {
    /// A name path element
    Name(&'a str),
    /// An index path element.
    Index(usize),
}

impl<'a> From<PathComponentRef<'a>> for PathComponent {
    fn from(value: PathComponentRef<'a>) -> Self {
        match value {
            PathComponentRef::Name(s) => PathComponent::OwnedName(s.to_owned()),
            PathComponentRef::Index(i) => PathComponent::Index(i),
        }
    }
}

/// A component of a path. This can be a name or a index.
///
/// In the case of a name it can either be an owned on `'static` string. Use [`Self::borrow`] to
/// access names uniformly.
#[derive(Clone, Debug, Eq)]
pub enum PathComponent {
    /// A name in the path (unowned string).
    Name(&'static str),
    /// A name in the path (owned string).
    OwnedName(String),
    /// A number in the path.
    Index(usize),
}

impl Display for PathComponent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.borrow() {
            PathComponentRef::Name(n) => f.write_str(n),
            PathComponentRef::Index(i) => write!(f, "[{}]", i),
        }
    }
}

impl PartialEq for PathComponent {
    fn eq(&self, other: &Self) -> bool {
        self.borrow() == other.borrow()
    }
}

impl Hash for PathComponent {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.borrow().hash(state);
    }
}

impl PathComponent {
    /// Returns a ref (-like) value which provides uniform access to names as a `&str`.
    pub fn borrow(&self) -> PathComponentRef {
        match self {
            PathComponent::Name(s) => PathComponentRef::Name(s),
            PathComponent::OwnedName(s) => PathComponentRef::Name(s),
            PathComponent::Index(i) => PathComponentRef::Index(*i),
        }
    }
}

/// A path consisting of multiple components. For example, "a.b" or "x[2].y".
#[derive(Debug, PartialEq, Eq, Clone, Default, Hash)]
pub struct Path {
    components: Vec<PathComponent>,
}

impl From<&Path> for Path {
    fn from(value: &Path) -> Self {
        value.clone()
    }
}

impl Display for Path {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for c in self.components.iter() {
            if !first && matches!(c.borrow(), PathComponentRef::Name(_)) {
                f.write_str(".")?;
            }
            first = false;
            c.fmt(f)?;
        }
        Ok(())
    }
}

impl Path {
    /// Create a new path with the given components.
    ///
    /// For internal use only.
    pub fn new(components: Vec<PathComponent>) -> Self {
        Self { components }
    }

    /// Concatenate `self` and `other` into one longer path.
    pub fn append(&self, other: &Path) -> Path {
        Path::new(
            self.components
                .iter()
                .chain(other.components.iter())
                .cloned()
                .collect(),
        )
    }

    #[cfg(ktest)]
    /// Create an arbitrary path to use for testing.
    pub fn test() -> Path {
        Path::new(vec![PathComponent::Name("TESTING_OQUEUE")])
    }
}

/// A pattern for matching against a path component.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PathComponentPattern {
    /// Matches a specific path component exactly.
    Fixed(PathComponent),
    /// Matches any name component.
    AnyName,
    /// Matches any index component.
    AnyIndex,
}

impl Display for PathComponentPattern {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PathComponentPattern::Fixed(c) => c.fmt(f),
            PathComponentPattern::AnyName => f.write_str("*"),
            PathComponentPattern::AnyIndex => f.write_str("[*]"),
        }
    }
}

impl PathComponentPattern {
    /// Checks if this pattern matches the given path component.
    pub fn matches(&self, component: &PathComponent) -> bool {
        match (self, component) {
            (PathComponentPattern::Fixed(pat), v) => pat == v,
            (
                PathComponentPattern::AnyName,
                PathComponent::Name(_) | PathComponent::OwnedName(_),
            ) => true,
            (PathComponentPattern::AnyIndex, PathComponent::Index(_)) => true,
            _ => false,
        }
    }
}

/// A pattern for matching against a path.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct PathPattern {
    components: Vec<PathComponentPattern>,
}

impl Display for PathPattern {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let mut first = true;
        for c in self.components.iter() {
            if !first
                && matches!(
                    c,
                    PathComponentPattern::Fixed(
                        PathComponent::Name(_) | PathComponent::OwnedName(_)
                    ) | PathComponentPattern::AnyName
                )
            {
                f.write_str(".")?;
            }
            first = false;
            c.fmt(f)?;
        }
        Ok(())
    }
}

impl PathPattern {
    /// Create a new path pattern with the given components.
    ///
    /// For internal use only.
    pub fn new(components: Vec<PathComponentPattern>) -> Self {
        Self { components }
    }

    /// Checks if `path` matches `self`.
    pub fn matches(&self, path: &Path) -> bool {
        if self.components.len() != path.components.len() {
            return false;
        }

        self.components
            .iter()
            .zip(&path.components)
            .all(|(pat, comp)| pat.matches(comp))
    }

    /// Return `Some(suffix)` if a prefix of `path` matches `self`.
    pub fn matches_prefix(&self, path: &Path) -> Option<Path> {
        let matched_len = self
            .components
            .iter()
            .zip(&path.components)
            .take_while(|(pat, comp)| pat.matches(comp))
            .count();

        if matched_len == self.components.len() {
            Some(Path {
                components: path.components[matched_len..].to_vec(),
            })
        } else {
            None
        }
    }

    /// Return `Some(prefix)` if a suffix of `path` matches `self`.
    pub fn matches_suffix(&self, path: &Path) -> Option<Path> {
        let matched_len = self
            .components
            .iter()
            .rev()
            .zip(path.components.iter().rev())
            .take_while(|(pat, comp)| pat.matches(comp))
            .count();

        if matched_len == self.components.len() {
            Some(Path {
                components: path.components[..path.components.len() - matched_len].to_vec(),
            })
        } else {
            None
        }
    }
}

/// Creates a new [`Path`] from `.` (dot) delimited literal. The syntax is a series of segments
/// which are either a name, like `a`, or a name and index, like `b[5]`. The index can be replaced
/// by `{expr}` to set the index at runtime. The index can also be replaced by the keyword `unique`
/// to generate a new unique number for each invocation of this `path!` invocation. If there are
/// multiple identical `path!` invocations in the program, they will produce duplicate indexes.
/// Internally, the unique index is generated by an static atomic counter local to the macro call
/// site.
///
/// The returned path will be as static as possible and does not require parsing at runtime.
///
/// # Examples
/// ```
/// let path = path!(a.b[3].j);
/// assert_eq!(path.to_string(), "a.b[3].j");
/// let index = 42;
/// let path = path!(x[unique].n[{index}]);
/// ```
#[macro_export]
macro_rules! path {
    ($($part:tt)*) => {{
        #[allow(clippy::allow_attributes, unused)]
        use $crate::orpc::path::{Path, PathComponent};
        #[allow(clippy::allow_attributes, unused)]
        use ::core::sync::atomic::{AtomicUsize, Ordering};
        use ::alloc::vec;
        #[allow(clippy::allow_attributes, unused)]
        use ::alloc::string::ToString;

        let components = $crate::__path_parse!([] @ {$($part)*});
        Path::new(components)
    }};
}

/// Internal macro for parsing path components.
///
/// Arguments are: [output list...] @ { remaining unparsed tokens }
///
/// Each step the first segment in the unparsed tokens is converted and placed in the output list.
#[doc(hidden)]
#[macro_export]
macro_rules! __path_parse {
    // Empty
    ([$($ret:tt)*]
        @ {}) => {
        vec![$($ret)*]
    };

    // Process the first segment of the path.
    ([$($ret:tt)*]
        @ { $name:tt [$($index:tt)*] $(. $($rest:tt)*)? }) => {
        $crate::__path_parse!([
                $($ret)*
                $crate::__path_segment!(@name $name),
                $crate::__path_segment!(@index $($index)*),
            ]
            @ { $($($rest)*)? })
    };
    ([$($ret:tt)*]
        @ { $name:tt                 $(. $($rest:tt)*)? }) => {
        $crate::__path_parse!([
                $($ret)*
                $crate::__path_segment!(@name $name),
            ]
            @ { $($($rest)*)? })
    };

}

/// Convert names or indexes into the actual components.
#[doc(hidden)]
#[macro_export]
macro_rules! __path_segment {
    (@name {$name:expr}) => {
        PathComponent::OwnedName($name.to_string())
    };
    (@name $name:ident) => {
        PathComponent::Name(stringify!($name))
    };

    (@index {$index:expr}) => {
        PathComponent::Index($index)
    };
    (@index $index:literal) => {
        PathComponent::Index($index)
    };
    (@index unique) => {{
        static UNIQUE_COUNTER: AtomicUsize = AtomicUsize::new(0);
        PathComponent::Index(UNIQUE_COUNTER.fetch_add(1, Ordering::Relaxed))
    }};
}

/// Creates a new [`PathPattern`]. The returned pattern will be as static as possible and does not
/// require parsing at runtime.
///
/// # Examples
/// ```
/// let index = 42;
/// let pattern = path_pattern!(*[*].d[*].f.dynamic[{index}]);
/// assert_eq!(pattern.to_string(), "*[*].d[*].f.dynamic[42]");
/// ```
#[macro_export]
macro_rules! path_pattern {
    ($($part:tt)*) => {{
        #[allow(clippy::allow_attributes, unused)]
        use $crate::orpc::path::{PathPattern, PathComponentPattern, PathComponent};
        use ::alloc::vec;

        let components = $crate::__path_pattern_parse!([] @ {$($part)*});
        PathPattern::new(components)
    }};
}

/// Internal macro for parsing path pattern components.
///
/// Arguments are: [output list...] @ { remaining unparsed tokens }
///
/// Each step the first segment in the unparsed tokens is converted and placed in the output list.
#[doc(hidden)]
#[macro_export]
macro_rules! __path_pattern_parse {
    // Empty pattern
    ([$($ret:tt)*]
        @ {}) => {
        vec![$($ret)*]
    };

    ([$($ret:tt)*]
        @ { $name:tt [$($index:tt)*] $(. $($rest:tt)*)? }) => {
        $crate::__path_pattern_parse!([
                $($ret)*
                $crate::__path_pattern_segment!(@name $name),
                $crate::__path_pattern_segment!(@index $($index)*),
            ]
            @ { $($($rest)*)? })
    };
    ([$($ret:tt)*]
        @ { $name:tt                 $(. $($rest:tt)*)? }) => {
        $crate::__path_pattern_parse!([
                $($ret)*
                $crate::__path_pattern_segment!(@name $name),
            ]
            @ { $($($rest)*)? })
    };
}

/// Convert names or indexes into the actual components.
#[doc(hidden)]
#[macro_export]
macro_rules! __path_pattern_segment {
    (@name *) => {
        PathComponentPattern::AnyName
    };
    (@name {$name:expr}) => {
        PathComponentPattern::Fixed(PathComponent::OwnedName($name.to_string()))
    };
    (@name $name:ident) => {
        PathComponentPattern::Fixed(PathComponent::Name(stringify!($name)))
    };

    (@index *) => {
        PathComponentPattern::AnyIndex
    };
    (@index {$index:expr}) => {
        PathComponentPattern::Fixed(PathComponent::Index($index))
    };
    (@index $index:literal) => {
        PathComponentPattern::Fixed(PathComponent::Index($index))
    };
}

#[cfg(ktest)]
mod test {
    use alloc::string::ToString as _;
    use core::array;

    use super::*;
    use crate::prelude::*;

    #[ktest]
    fn test_path_display() {
        let path = path!(a.b[3].j);
        assert_eq!(path.to_string(), "a.b[3].j");
    }

    #[ktest]
    fn test_path_pattern_display() {
        let pattern = path_pattern!(*[*].d[*].f);
        assert_eq!(pattern.to_string(), "*[*].d[*].f");
    }

    #[ktest]
    fn test_path_component_display() {
        let name_component = PathComponent::Name("test");
        assert_eq!(name_component.to_string(), "test");

        let owned_name_component = PathComponent::OwnedName("owned".to_string());
        assert_eq!(owned_name_component.to_string(), "owned");

        let index_component = PathComponent::Index(42);
        assert_eq!(index_component.to_string(), "[42]");
    }

    #[ktest]
    fn test_path_component_pattern_display() {
        let fixed_name_pattern = PathComponentPattern::Fixed(PathComponent::Name("fixed"));
        assert_eq!(fixed_name_pattern.to_string(), "fixed");

        let any_name_pattern = PathComponentPattern::AnyName;
        assert_eq!(any_name_pattern.to_string(), "*");

        let any_index_pattern = PathComponentPattern::AnyIndex;
        assert_eq!(any_index_pattern.to_string(), "[*]");
    }

    #[ktest]
    fn test_path_component_equality() {
        let static_name = PathComponent::Name("test");
        let owned_name = PathComponent::OwnedName("test".to_string());
        assert_eq!(static_name, owned_name);

        let index1 = PathComponent::Index(42);
        let index2 = PathComponent::Index(42);
        assert_eq!(index1, index2);

        let different_index = PathComponent::Index(43);
        assert_ne!(index1, different_index);
    }

    #[ktest]
    fn test_path_component_pattern_matching() {
        let fixed_name = PathComponentPattern::Fixed(PathComponent::Name("test"));
        assert!(fixed_name.matches(&PathComponent::Name("test")));
        assert!(fixed_name.matches(&PathComponent::OwnedName("test".to_string())));

        let any_name = PathComponentPattern::AnyName;
        assert!(any_name.matches(&PathComponent::Name("any")));
        assert!(any_name.matches(&PathComponent::OwnedName("any".to_string())));
        assert!(!any_name.matches(&PathComponent::Index(1)));

        let any_index = PathComponentPattern::AnyIndex;
        assert!(any_index.matches(&PathComponent::Index(1)));
        assert!(!any_index.matches(&PathComponent::Name("test")));
    }

    #[ktest]
    fn test_path_pattern_matching() {
        let pattern = path_pattern!(a.b[*].d);

        assert!(pattern.matches(&path!(a.b[3].d)));
        assert!(!pattern.matches(&path!(a.c[3].d)));
        let pattern = path_pattern!(a.*[3].d);
        assert!(pattern.matches(&path!(a.b[3].d)));
        assert!(pattern.matches(&path!(a.c[3].d)));
        assert!(!pattern.matches(&path!(a.b[2].d)));
        assert!(!pattern.matches(&path!(a.b[3].e)));
    }

    #[ktest]
    fn test_path_pattern_prefix_matching() {
        let pattern = path_pattern!(a.*[3]);
        let path = path!(a.b[3].d[2]);
        let suffix = pattern.matches_prefix(&path);
        assert!(suffix.is_some());
        assert_eq!(suffix.unwrap(), path!(d[2]));
    }

    #[ktest]
    fn test_path_pattern_suffix_matching() {
        let pattern = path_pattern!(b[*].d);
        let path = path!(a.x[1].b[3].d);
        let prefix = pattern.matches_suffix(&path);
        assert!(prefix.is_some());
        assert_eq!(prefix.unwrap(), path!(a.x[1]));
    }

    #[ktest]
    fn test_path_concat() {
        let path1 = path!(a.b[1]);
        let path2 = path!(c.d[2]);
        let concatenated = path1.append(&path2);
        assert_eq!(concatenated, path!(a.b[1].c.d[2]));

        // Test with empty paths
        let empty = path!();
        assert_eq!(path1.append(&empty), path!(a.b[1]));
        assert_eq!(empty.append(&path2), path!(c.d[2]));
        assert_eq!(empty.append(&empty), path!());
    }

    #[ktest]
    fn test_unique_index() {
        let paths: [_; 2] = array::from_fn(|_| path!(a.b[unique]));
        assert_ne!(paths[0], paths[1]);
        assert!(path_pattern!(a.b[*]).matches(&paths[0]));
    }

    #[ktest]
    fn test_dynamic_index() {
        let index = 42;
        assert_eq!(path!(a[{ index }].b[{ 1 + 2 }]), path!(a[42].b[3]));
    }

    #[ktest]
    fn test_dynamic_index_pattern() {
        let index = 42;
        assert_eq!(
            path_pattern!(a[{ index }].b[{ 1 + 2 }]),
            path_pattern!(a[42].b[3])
        );
    }

    #[ktest]
    fn test_dynamic_name() {
        let name_a = "a";
        let name_b = "b".to_owned();
        let x = path!({name_a}.{name_b});
        assert_eq!(x, path!(a.b));
        assert_eq!(path!({name_a}[1].{name_b}[2]), path!(a[1].b[2]));
    }

    #[ktest]
    fn test_dynamic_name_pattern() {
        let name_a = "a";
        let name_b = "b".to_owned();
        assert_eq!(path_pattern!({name_a}.{name_b}), path_pattern!(a.b));
        assert_eq!(
            path_pattern!({name_a}[1].{name_b}[2]),
            path_pattern!(a[1].b[2])
        );
    }
}
