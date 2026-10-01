// SPDX-License-Identifier: MPL-2.0

use aster_bigtcp::wire::{Ipv4Address, Ipv6Address, PortNum};
use ostd::orpc::framework::projection::DefaultProjection;
use serde::Serialize;

use crate::{
    net::socket::{netlink::NetlinkSocketAddr, unix::UnixSocketAddr, vsock::VsockSocketAddr},
    prelude::*,
};

#[derive(Debug, Eq, PartialEq)]
pub enum SocketAddr {
    Unix(UnixSocketAddr),
    IPv4(Ipv4Address, PortNum),
    IPv6(Ipv6Address, PortNum),
    Netlink(NetlinkSocketAddr),
    Vsock(VsockSocketAddr),
}

impl DefaultProjection for SocketAddr {
    type Projected = SocketAddrProjected;

    fn project(&self) -> Self::Projected {
        match self {
            SocketAddr::Unix(_) => SocketAddrProjected::Unix,
            SocketAddr::IPv4(ipv4_addr, port) => SocketAddrProjected::IPv4(*ipv4_addr, *port),
            SocketAddr::Netlink(netlink_socket_addr) => SocketAddrProjected::Netlink(*netlink_socket_addr),
            SocketAddr::Vsock(vsock_socket_addr) => SocketAddrProjected::Vsock(*vsock_socket_addr),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
pub enum SocketAddrProjected {
    Unix,
    IPv4(Ipv4Address, PortNum),
    Netlink(NetlinkSocketAddr),
    Vsock(VsockSocketAddr),
}
