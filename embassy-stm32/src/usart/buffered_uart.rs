use super::{Instance, Uart};
use crate::dma::NoDma;
use atomic_polyfill::{compiler_fence, Ordering};
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::Context;
use core::task::Poll;
use embassy::util::Unborrow;
use embassy::waitqueue::WakerRegistration;
use embassy_hal_common::peripheral::{PeripheralMutex, PeripheralState, StateStorage};
use embassy_hal_common::ring_buffer::RingBuffer;
use embassy_hal_common::unborrow;

pub struct State<'d, T: Instance>(StateStorage<StateInner<'d, T>>);
impl<'d, T: Instance> State<'d, T> {
    pub fn new() -> Self {
        Self(StateStorage::new())
    }
}

pub struct StateInner<'d, T: Instance> {
    uart: Uart<'d, T, NoDma, NoDma>,
    phantom: PhantomData<&'d mut T>,

    rx_waker: WakerRegistration,
    rx: RingBuffer<'d>,

    tx_waker: WakerRegistration,
    tx: RingBuffer<'d>,
}

unsafe impl<'d, T: Instance> Send for StateInner<'d, T> {}
unsafe impl<'d, T: Instance> Sync for StateInner<'d, T> {}

pub struct BufferedUart<'d, T: Instance> {
    inner: PeripheralMutex<'d, StateInner<'d, T>>,
}

impl<'d, T: Instance> Unpin for BufferedUart<'d, T> {}

impl<'d, T: Instance> BufferedUart<'d, T> {
    pub unsafe fn new(
        state: &'d mut State<'d, T>,
        uart: Uart<'d, T, NoDma, NoDma>,
        irq: impl Unborrow<Target = T::Interrupt> + 'd,
        tx_buffer: &'d mut [u8],
        rx_buffer: &'d mut [u8],
    ) -> BufferedUart<'d, T> {
        unborrow!(irq);

        let r = uart.inner.regs();
        r.cr1().modify(|w| {
            w.set_rxneie(true);
            w.set_idleie(true);
        });

        Self {
            inner: PeripheralMutex::new_unchecked(irq, &mut state.0, move || StateInner {
                uart,
                phantom: PhantomData,
                tx: RingBuffer::new(tx_buffer),
                tx_waker: WakerRegistration::new(),

                rx: RingBuffer::new(rx_buffer),
                rx_waker: WakerRegistration::new(),
            }),
        }
    }
}

impl<'d, T: Instance> PeripheralState for StateInner<'d, T>
where
    Self: 'd,
{
    type Interrupt = T::Interrupt;
    fn on_interrupt(&mut self) {
        super::on_buffered_rx(&mut self.uart, &mut self.rx, &mut self.rx_waker);
        super::on_buffered_tx(&mut self.uart, &mut self.tx, &mut self.tx_waker);
    }
}

impl<'d, T: Instance> embassy::io::AsyncBufRead for BufferedUart<'d, T> {
    fn poll_fill_buf(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<&[u8], embassy::io::Error>> {
        self.inner.with(|state| {
            compiler_fence(Ordering::SeqCst);

            // We have data ready in buffer? Return it.
            let buf = state.rx.pop_buf();
            if !buf.is_empty() {
                let buf: &[u8] = buf;
                // Safety: buffer lives as long as uart
                let buf: &[u8] = unsafe { core::mem::transmute(buf) };
                return Poll::Ready(Ok(buf));
            }

            state.rx_waker.register(cx.waker());
            Poll::<Result<&[u8], embassy::io::Error>>::Pending
        })
    }
    fn consume(mut self: Pin<&mut Self>, amt: usize) {
        let signal = self.inner.with(|state| {
            let full = state.rx.is_full();
            state.rx.pop(amt);
            full
        });
        if signal {
            self.inner.pend();
        }
    }
}

impl<'d, T: Instance> embassy::io::AsyncWrite for BufferedUart<'d, T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, embassy::io::Error>> {
        let (poll, empty) = self.inner.with(|state| {
            let empty = state.tx.is_empty();
            let tx_buf = state.tx.push_buf();
            if tx_buf.is_empty() {
                state.tx_waker.register(cx.waker());
                return (Poll::Pending, empty);
            }

            let n = core::cmp::min(tx_buf.len(), buf.len());
            tx_buf[..n].copy_from_slice(&buf[..n]);
            state.tx.push(n);

            (Poll::Ready(Ok(n)), empty)
        });
        if empty {
            self.inner.pend();
        }
        poll
    }
}
