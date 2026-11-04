// SPDX-License-Identifier: MPL-2.0

use keyable_arc::KeyableArc;
use ostd::DefaultProjection;

use super::ns::{self, AbstractHandle};
use crate::{
    fs::vfs::{inode::Inode, path::Path},
    net::socket::util::SocketAddr,
    prelude::*,
};

#[derive(Clone, Debug, Eq, PartialEq, DefaultProjection)]
pub(crate) enum UnixSocketAddr {
    Unnamed,
    Path(#[projection(opaque)] Arc<CStr>),
    Abstract(#[projection(opaque)] Arc<[u8]>),
}

impl UnixSocketAddr {
    pub(super) fn bind(self) -> Result<UnixSocketAddrBound> {
        let bound = match self {
            Self::Unnamed => UnixSocketAddrBound::Abstract(ns::alloc_ephemeral_abstract_name()?),
            Self::Path(path_name) => {
                let path = ns::create_socket_file(&path_name.to_string_lossy())?;
                UnixSocketAddrBound::Path(path_name, path)
            }
            Self::Abstract(name) => UnixSocketAddrBound::Abstract(ns::create_abstract_name(name)?),
        };

        Ok(bound)
    }

    pub(super) fn bind_unnamed(&self) -> Result<()> {
        if matches!(self, UnixSocketAddr::Unnamed) {
            Ok(())
        } else {
            return_errno_with_message!(Errno::EINVAL, "the socket is already bound");
        }
    }

    pub(super) fn connect(&self) -> Result<UnixSocketAddrKey> {
        let bound = match self {
            Self::Unnamed => return_errno_with_message!(
                Errno::EINVAL,
                "the unnamed UNIX domain socket address is not valid for connecting"
            ),
            Self::Path(path) => UnixSocketAddrKey::Path(KeyableArc::from(
                ns::lookup_socket_file(&path.to_string_lossy())?
                    .inode()
                    .clone(),
            )),
            Self::Abstract(name) => {
                UnixSocketAddrKey::Abstract(KeyableArc::from(ns::lookup_abstract_name(name)?))
            }
        };

        Ok(bound)
    }
}

impl TryFrom<SocketAddr> for UnixSocketAddr {
    type Error = Error;

    fn try_from(value: SocketAddr) -> Result<Self> {
        match value {
            SocketAddr::Unix(unix_socket_addr) => Ok(unix_socket_addr),
            _ => return_errno_with_message!(
                Errno::EINVAL,
                "the socket address is not a valid UNIX domain socket address"
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum UnixSocketAddrBound {
    Path(Arc<CStr>, Path),
    Abstract(Arc<AbstractHandle>),
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) enum UnixSocketAddrKey {
    Path(KeyableArc<dyn Inode>),
    Abstract(KeyableArc<AbstractHandle>),
}

impl UnixSocketAddrBound {
    pub(super) fn to_key(&self) -> UnixSocketAddrKey {
        match self {
            Self::Path(_, path) => UnixSocketAddrKey::Path(KeyableArc::from(path.inode().clone())),
            Self::Abstract(handle) => UnixSocketAddrKey::Abstract(KeyableArc::from(handle.clone())),
        }
    }
}

impl From<UnixSocketAddrBound> for UnixSocketAddr {
    fn from(value: UnixSocketAddrBound) -> Self {
        match value {
            UnixSocketAddrBound::Path(path, _) => Self::Path(path),
            UnixSocketAddrBound::Abstract(name) => Self::Abstract(name.name()),
        }
    }
}

impl From<Option<UnixSocketAddrBound>> for UnixSocketAddr {
    fn from(value: Option<UnixSocketAddrBound>) -> Self {
        match value {
            Some(addr) => addr.into(),
            None => Self::Unnamed,
        }
    }
}

impl<T: Into<UnixSocketAddr>> From<T> for SocketAddr {
    fn from(value: T) -> Self {
        SocketAddr::Unix(value.into())
    }
}

#[cfg(all(ktest, not(baseline_asterinas)))]
mod test {
    use ostd::{orpc::oqueue::default_projection::DefaultProjection as _, prelude::ktest};

    use super::*;

    #[ktest]
    fn path_projection_identifies_the_shared_allocation() {
        let path: Arc<str> = Arc::from("socket");
        let shared = UnixSocketAddr::Path(path.clone()).default_projection();
        let same = UnixSocketAddr::Path(path.clone()).default_projection();
        let distinct = UnixSocketAddr::Path(Arc::from("socket")).default_projection();

        let UnixSocketAddrDefaultProjection::Path(shared_id) = shared else {
            panic!("expected path projection");
        };
        let UnixSocketAddrDefaultProjection::Path(same_id) = same else {
            panic!("expected path projection");
        };
        let UnixSocketAddrDefaultProjection::Path(distinct_id) = distinct else {
            panic!("expected path projection");
        };
        assert_eq!(shared_id, same_id);
        assert_ne!(shared_id, distinct_id);
    }
}
