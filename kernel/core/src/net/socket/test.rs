// SPDX-License-Identifier: MPL-2.0

use core::net::Ipv6Addr;

use ostd::{
    orpc::oqueue::{OQueueBase, ObservationQuery, registry::lookup_by_path},
    path,
    prelude::ktest,
};

use super::{Socket, ip::StreamSocket, socket, unix::UnixDatagramSocket};
use crate::{
    error::Errno,
    net::socket::{ip::IpAddressFamily, util::SocketAddr},
};

/// Tests that two socket implementations share the same method OQueues.
#[ktest]
fn socket_method_traces_call_and_return() {
    let stream_socket = StreamSocket::new(true, IpAddressFamily::IPv4);
    let datagram_socket = UnixDatagramSocket::new(true);
    let bind_address_fn = || SocketAddr::IPv6(Ipv6Addr::LOCALHOST, 0);
    assert_eq!(
        stream_socket.bind(bind_address_fn()).unwrap_err().error(),
        Errno::EAFNOSUPPORT
    );

    let call_queue = lookup_by_path::<socket::BindCallDescriptor>(&path!(
        aster_core.net.socket.Socket.bind.call
    ))
    .expect("Socket::bind call OQueue should exist");
    let return_queue = lookup_by_path::<socket::BindReturnDescriptor>(&path!(
        aster_core.net.socket.Socket.bind.ret
    ))
    .expect("Socket::bind return OQueue should exist");

    let stream_inode = stream_socket.common().path().inode().ino();
    let datagram_inode = datagram_socket.common().path().inode().ino();
    let call_observer = call_queue
        .attach_weak_observer(
            2,
            ObservationQuery::new(|event: &socket::BindCall| {
                (
                    event.self_.common().path().inode().ino(),
                    event._socket_addr == &SocketAddr::IPv6(Ipv6Addr::LOCALHOST, 0),
                )
            }),
        )
        .unwrap();
    let return_observer = return_queue
        .attach_weak_observer(
            2,
            ObservationQuery::new(|event: &socket::BindReturn| {
                (
                    event.self_.common().path().inode().ino(),
                    event.value.as_ref().err().map(|error| error.error()),
                )
            }),
        )
        .unwrap();

    assert_eq!(
        stream_socket.bind(bind_address_fn()).unwrap_err().error(),
        Errno::EAFNOSUPPORT
    );
    assert_eq!(
        datagram_socket.bind(bind_address_fn()).unwrap_err().error(),
        Errno::EINVAL
    );
    assert_eq!(
        call_observer.weak_observe_recent(2).unwrap(),
        [Some((stream_inode, true)), Some((datagram_inode, true))]
    );
    assert_eq!(
        return_observer.weak_observe_recent(2).unwrap(),
        [
            Some((stream_inode, Some(Errno::EAFNOSUPPORT))),
            Some((datagram_inode, Some(Errno::EINVAL))),
        ]
    );
}
