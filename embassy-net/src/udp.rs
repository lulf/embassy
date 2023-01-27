use core::cell::RefCell;
use core::future::poll_fn;
use core::mem;
use core::task::Poll;

use embassy_net_driver::Driver;
use embedded_io::ErrorKind;
use smoltcp::iface::{Interface, SocketHandle};
use smoltcp::socket::udp::{self, PacketMetadata};
use smoltcp::wire::{IpEndpoint, IpListenEndpoint};

use crate::{SocketStack, Stack};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum BindError {
    /// The socket was already open.
    InvalidState,
    /// No route to host.
    NoRoute,
}

impl embedded_io::Error for BindError {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error {
    /// No route to host.
    NoRoute,
}

impl embedded_io::Error for Error {
    fn kind(&self) -> ErrorKind {
        ErrorKind::Other
    }
}

pub struct UdpSocket<'a> {
    stack: &'a RefCell<SocketStack>,
    handle: SocketHandle,
}

impl<'a> UdpSocket<'a> {
    pub fn new<D: Driver>(
        stack: &'a Stack<D>,
        rx_meta: &'a mut [PacketMetadata],
        rx_buffer: &'a mut [u8],
        tx_meta: &'a mut [PacketMetadata],
        tx_buffer: &'a mut [u8],
    ) -> Self {
        let s = &mut *stack.socket.borrow_mut();

        let rx_meta: &'static mut [PacketMetadata] = unsafe { mem::transmute(rx_meta) };
        let rx_buffer: &'static mut [u8] = unsafe { mem::transmute(rx_buffer) };
        let tx_meta: &'static mut [PacketMetadata] = unsafe { mem::transmute(tx_meta) };
        let tx_buffer: &'static mut [u8] = unsafe { mem::transmute(tx_buffer) };
        let handle = s.sockets.add(udp::Socket::new(
            udp::PacketBuffer::new(rx_meta, rx_buffer),
            udp::PacketBuffer::new(tx_meta, tx_buffer),
        ));

        Self {
            stack: &stack.socket,
            handle,
        }
    }

    pub fn bind<T>(&mut self, endpoint: T) -> Result<(), BindError>
    where
        T: Into<IpListenEndpoint>,
    {
        let mut endpoint = endpoint.into();

        if endpoint.port == 0 {
            // If user didn't specify port allocate a dynamic port.
            endpoint.port = self.stack.borrow_mut().get_local_port();
        }

        match self.with_mut(|s, _| s.bind(endpoint)) {
            Ok(()) => Ok(()),
            Err(udp::BindError::InvalidState) => Err(BindError::InvalidState),
            Err(udp::BindError::Unaddressable) => Err(BindError::NoRoute),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&udp::Socket, &Interface) -> R) -> R {
        let s = &*self.stack.borrow();
        let socket = s.sockets.get::<udp::Socket>(self.handle);
        f(socket, &s.iface)
    }

    fn with_mut<R>(&self, f: impl FnOnce(&mut udp::Socket, &mut Interface) -> R) -> R {
        let s = &mut *self.stack.borrow_mut();
        let socket = s.sockets.get_mut::<udp::Socket>(self.handle);
        let res = f(socket, &mut s.iface);
        s.waker.wake();
        res
    }

    pub async fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, IpEndpoint), Error> {
        poll_fn(move |cx| {
            self.with_mut(|s, _| match s.recv_slice(buf) {
                Ok(x) => Poll::Ready(Ok(x)),
                // No data ready
                Err(udp::RecvError::Exhausted) => {
                    //s.register_recv_waker(cx.waker());
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            })
        })
        .await
    }

    pub async fn send_to<T>(&self, buf: &[u8], remote_endpoint: T) -> Result<(), Error>
    where
        T: Into<IpEndpoint>,
    {
        let remote_endpoint = remote_endpoint.into();
        poll_fn(move |cx| {
            self.with_mut(|s, _| match s.send_slice(buf, remote_endpoint) {
                // Entire datagram has been sent
                Ok(()) => Poll::Ready(Ok(())),
                Err(udp::SendError::BufferFull) => {
                    s.register_send_waker(cx.waker());
                    Poll::Pending
                }
                Err(udp::SendError::Unaddressable) => Poll::Ready(Err(Error::NoRoute)),
            })
        })
        .await
    }

    pub fn endpoint(&self) -> IpListenEndpoint {
        self.with(|s, _| s.endpoint())
    }

    pub fn is_open(&self) -> bool {
        self.with(|s, _| s.is_open())
    }

    pub fn close(&mut self) {
        self.with_mut(|s, _| s.close())
    }

    pub fn may_send(&self) -> bool {
        self.with(|s, _| s.can_send())
    }

    pub fn may_recv(&self) -> bool {
        self.with(|s, _| s.can_recv())
    }
}

impl Drop for UdpSocket<'_> {
    fn drop(&mut self) {
        self.stack.borrow_mut().sockets.remove(self.handle);
    }
}

#[cfg(all(feature = "unstable-traits", feature = "nightly"))]
pub mod client {
    use core::cell::UnsafeCell;
    use core::mem::MaybeUninit;
    use core::ptr::NonNull;

    use atomic_polyfill::{AtomicBool, Ordering};
    use embedded_nal_async::{ConnectedUdp, IpAddr, SocketAddr, UnconnectedUdp};

    use super::*;

    pub struct UdpStack<'d, D: Driver> {
        stack: &'d Stack<D>,
    }

    impl<'a, D> embedded_nal_async::UdpStack for UdpStack<'a, D>
    where
        D: Driver,
    {
        type Error = Error;

        type Connected = UdpSocket<'a>;
        type UniquelyBound = UdpSocket<'a>;
        type MultiplyBound = UdpSocket<'a>;

        async fn connect_from(
            &self,
            local: SocketAddr,
            remote: SocketAddr,
        ) -> Result<(SocketAddr, Self::Connected), Self::Error> {
            todo!()
        }

        async fn bind_single(&self, _: SocketAddr) -> Result<(SocketAddr, Self::UniquelyBound), Self::Error> {
            todo!()
        }
        async fn bind_multiple(&self, _: SocketAddr) -> Result<Self::MultiplyBound, Self::Error> {
            todo!()
        }
    }

    impl<'a> ConnectedUdp for UdpSocket<'a> {
        type Error = Error;

        async fn send(&mut self, _: &[u8]) -> Result<(), Self::Error> {
            todo!()
        }
        async fn receive_into(&mut self, _: &mut [u8]) -> Result<usize, Self::Error> {
            todo!()
        }
    }

    impl<'a> UnconnectedUdp for UdpSocket<'a> {
        type Error = Error;
        async fn send(&mut self, _: SocketAddr, _: SocketAddr, _: &[u8]) -> Result<(), Self::Error> {
            todo!()
        }
        async fn receive_into(&mut self, _: &mut [u8]) -> Result<(usize, SocketAddr, SocketAddr), Self::Error> {
            todo!()
        }
    }
}
