use atomic_polyfill::{compiler_fence, Ordering};
use core::future::Future;
use core::marker::PhantomData;
use embassy::interrupt::InterruptExt;
use embassy::util::{Unborrow, WakerRegistration};
use embassy_hal_common::peripheral::{PeripheralMutex, PeripheralState, StateStorage};
use embassy_hal_common::ring_buffer::RingBuffer;
use embassy_hal_common::unborrow;
use futures::TryFutureExt;

use super::*;
use crate::dma::NoDma;
use crate::pac::usart::{regs, vals};

pub struct Uart<'d, T: Instance, TxDma = NoDma, RxDma = NoDma> {
    inner: T,
    phantom: PhantomData<&'d mut T>,
    tx_dma: TxDma,
    rx_dma: RxDma,
}

impl<'d, T: Instance, TxDma, RxDma> Uart<'d, T, TxDma, RxDma> {
    pub fn new(
        inner: impl Unborrow<Target = T>,
        rx: impl Unborrow<Target = impl RxPin<T>>,
        tx: impl Unborrow<Target = impl TxPin<T>>,
        tx_dma: impl Unborrow<Target = TxDma>,
        rx_dma: impl Unborrow<Target = RxDma>,
        config: Config,
    ) -> Self {
        unborrow!(inner, rx, tx, tx_dma, rx_dma);

        T::enable();
        let pclk_freq = T::frequency();

        // TODO: better calculation, including error checking and OVER8 if possible.
        let div = (pclk_freq.0 + (config.baudrate / 2)) / config.baudrate;

        let r = inner.regs();

        unsafe {
            rx.set_as_af(rx.af_num());
            tx.set_as_af(tx.af_num());

            r.cr2().write(|_w| {});
            r.cr3().write(|_w| {});

            r.brr().write(|w| w.set_brr(div as u16));
            r.cr1().write(|w| {
                w.set_ue(true);
                w.set_te(true);
                w.set_re(true);
                w.set_m0(vals::M0::BIT8);
                w.set_m1(vals::M1::M0);
                w.set_pce(config.parity != Parity::ParityNone);
                w.set_ps(match config.parity {
                    Parity::ParityOdd => vals::Ps::ODD,
                    Parity::ParityEven => vals::Ps::EVEN,
                    _ => vals::Ps::EVEN,
                });
            });
            r.cr2().write(|_w| {});
            r.cr3().write(|_w| {});
        }

        Self {
            inner,
            phantom: PhantomData,
            tx_dma,
            rx_dma,
        }
    }

    async fn write_dma(&mut self, buffer: &[u8]) -> Result<(), Error>
    where
        TxDma: crate::usart::TxDma<T>,
    {
        let ch = &mut self.tx_dma;
        unsafe {
            self.inner.regs().cr3().modify(|reg| {
                reg.set_dmat(true);
            });
        }
        let r = self.inner.regs();
        let dst = r.tdr().ptr() as *mut u8;
        ch.write(ch.request(), buffer, dst).await;
        Ok(())
    }

    async fn read_dma(&mut self, buffer: &mut [u8]) -> Result<(), Error>
    where
        RxDma: crate::usart::RxDma<T>,
    {
        let ch = &mut self.rx_dma;
        unsafe {
            self.inner.regs().cr3().modify(|reg| {
                reg.set_dmar(true);
            });
        }
        let r = self.inner.regs();
        let src = r.rdr().ptr() as *mut u8;
        ch.read(ch.request(), src, buffer).await;
        Ok(())
    }

    pub fn read_blocking(&mut self, buffer: &mut [u8]) -> Result<(), Error> {
        unsafe {
            let r = self.inner.regs();
            for b in buffer {
                loop {
                    let sr = r.isr().read();
                    if sr.pe() {
                        r.rdr().read();
                        return Err(Error::Parity);
                    } else if sr.fe() {
                        r.rdr().read();
                        return Err(Error::Framing);
                    } else if sr.nf() {
                        r.rdr().read();
                        return Err(Error::Noise);
                    } else if sr.ore() {
                        r.rdr().read();
                        return Err(Error::Overrun);
                    } else if sr.rxne() {
                        break;
                    }
                }
                *b = r.rdr().read().0 as u8;
            }
        }
        Ok(())
    }
}

impl<'d, T: Instance, RxDma> embedded_hal::blocking::serial::Write<u8>
    for Uart<'d, T, NoDma, RxDma>
{
    type Error = Error;
    fn bwrite_all(&mut self, buffer: &[u8]) -> Result<(), Self::Error> {
        unsafe {
            let r = self.inner.regs();
            for &b in buffer {
                while !r.isr().read().txe() {}
                r.tdr().write_value(regs::Dr(b as u32))
            }
        }
        Ok(())
    }
    fn bflush(&mut self) -> Result<(), Self::Error> {
        unsafe {
            let r = self.inner.regs();
            while !r.isr().read().tc() {}
        }
        Ok(())
    }
}

// rustfmt::skip because intellij removes the 'where' claus on the associated type.
impl<'d, T: Instance, TxDma, RxDma> embassy_traits::uart::Write for Uart<'d, T, TxDma, RxDma>
where
    TxDma: crate::usart::TxDma<T>,
{
    // rustfmt::skip because rustfmt removes the 'where' claus on the associated type.
    #[rustfmt::skip]
    type WriteFuture<'a> where Self: 'a = impl Future<Output = Result<(), embassy_traits::uart::Error>>;

    fn write<'a>(&'a mut self, buf: &'a [u8]) -> Self::WriteFuture<'a> {
        self.write_dma(buf)
            .map_err(|_| embassy_traits::uart::Error::Other)
    }
}

impl<'d, T: Instance, TxDma, RxDma> embassy_traits::uart::Read for Uart<'d, T, TxDma, RxDma>
where
    RxDma: crate::usart::RxDma<T>,
{
    // rustfmt::skip because rustfmt removes the 'where' claus on the associated type.
    #[rustfmt::skip]
    type ReadFuture<'a> where Self: 'a = impl Future<Output = Result<(), embassy_traits::uart::Error>>;

    fn read<'a>(&'a mut self, buf: &'a mut [u8]) -> Self::ReadFuture<'a> {
        self.read_dma(buf)
            .map_err(|_| embassy_traits::uart::Error::Other)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum RxState {
    Idle,
    Receiving(usize, *mut [u8]),
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum TxState<'a> {
    Idle,
    Transmitting(usize, &'a [u8]),
}

pub struct State<'d, T: Instance>(StateStorage<StateInner<'d, T>>);
impl<'d, T: Instance> State<'d, T> {
    pub fn new() -> Self {
        Self(StateStorage::new())
    }
}

pub struct StateInner<'d, T: Instance> {
    inner: T,
    phantom: PhantomData<&'d mut T>,

    rx_state: RxState,
    rx_waker: WakerRegistration,
    rx: RingBuffer<'d>,

    tx_state: TxState<'d>,
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
        irq.disable();
        irq.pend();

        let inner = uart.inner;

        irq.disable();
        irq.pend();

        let r = inner.regs();
        r.cr1().modify(|w| {
            w.set_rxneie(true);
            w.set_idleie(true);
        });

        Self {
            inner: PeripheralMutex::new_unchecked(irq, &mut state.0, move || StateInner {
                inner,
                phantom: PhantomData,
                tx: RingBuffer::new(tx_buffer),
                tx_waker: WakerRegistration::new(),
                tx_state: TxState::Idle,

                rx: RingBuffer::new(rx_buffer),
                rx_waker: WakerRegistration::new(),
                rx_state: RxState::Idle,
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
        let r = self.inner.regs();
        loop {
            match self.rx_state {
                RxState::Idle => {
                    //info!("  irq_rx: in state idle");

                    let buf = self.rx.push_buf();
                    if !buf.is_empty() {
                        // trace!("  irq_rx: starting {:?}", buf.len());
                        self.rx_state = RxState::Receiving(0, buf);
                    }
                    break;
                }
                RxState::Receiving(mut read, buf) => {
                    let mut done = false;
                    unsafe {
                        let bufs = &mut *buf;
                        loop {
                            let sr = r.isr().read();
                            if sr.pe() {
                                info!("Parity error");
                                done = true;
                                break;
                            } else if sr.fe() {
                                r.icr().write(|w| {
                                    w.set_fe(true);
                                });
                                done = true;
                                break;
                            } else if sr.nf() {
                                r.icr().write(|w| {
                                    w.set_nf(true);
                                });
                                //info!("Noise error");
                                done = true;
                                break;
                            } else if sr.ore() {
                                r.icr().write(|w| {
                                    w.set_ore(true);
                                });
                                //info!("overrun error");
                                done = true;
                                break;
                            } else if sr.rxne() {
                                let b = r.rdr().read().0 as u8;
                                //info!("RX {}", b);
                                bufs[read] = b;
                                read += 1;
                                if read == bufs.len() {
                                    //info!("RX of {} bytes done: {}", read, b);
                                    self.rx.push(read);
                                    self.rx_waker.wake();
                                    self.rx_state = RxState::Idle;
                                    break;
                                } else {
                                    self.rx_state = RxState::Receiving(read, buf);
                                }
                            } else if sr.idle() {
                                //info!("idle line detected, read {}", read);
                                r.icr().write(|w| {
                                    w.set_idle(true);
                                });
                                // Restart request with new buffer if we got data before idle
                                if read > 0 {
                                    self.rx.push(read);
                                    self.rx_waker.wake();
                                    self.rx_state = RxState::Idle;
                                } else {
                                    self.rx_state = RxState::Receiving(read, buf);
                                    done = true;
                                }
                                break;
                            } else {
                                done = true;
                                break;
                            }
                        }
                    }
                    if done {
                        break;
                    }
                }
            }
        }

        loop {
            match self.tx_state {
                TxState::Idle => {
                    unsafe {
                        if r.isr().read().txe() {
                            let buf = self.tx.pop_buf();
                            if !buf.is_empty() {
                                r.cr1().modify(|w| {
                                    w.set_txeie(true);
                                });
                                r.tdr().write_value(regs::Dr(buf[0].into()));
                                // Safety: buffer has the same lifetime as self
                                self.tx_state = TxState::Transmitting(1, core::mem::transmute(buf));
                            }
                        }
                    }
                    break;
                }
                TxState::Transmitting(written, buf) => {
                    unsafe {
                        if r.isr().read().txe() {
                            if written < buf.len() {
                                //info!("TX {}", buf[written]);
                                r.tdr().write_value(regs::Dr(buf[written].into()));
                                self.tx_state = TxState::Transmitting(written + 1, buf);
                                break;
                            } else {
                                //info!("TX DONE {}", written);
                                r.cr1().modify(|w| {
                                    w.set_txeie(false);
                                });
                                self.tx.pop(written);
                                self.tx_waker.wake();
                                self.tx_state = TxState::Idle;
                            }
                        }
                    }
                }
            }
        }
    }
}

use core::pin::Pin;
use core::task::Context;
use core::task::Poll;

impl<'d, T: Instance> embassy::io::AsyncBufRead for BufferedUart<'d, T> {
    fn poll_fill_buf(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<&[u8], embassy::io::Error>> {
        //info!("Buffered poll fill buf");
        self.inner.with(|state| {
            compiler_fence(Ordering::SeqCst);

            // We have data ready in buffer? Return it.
            let buf = state.rx.pop_buf();
            if !buf.is_empty() {
                //trace!("  got {} bytes in poll", buf.len());
                let buf: &[u8] = buf;
                // Safety: buffer lives as long as uart
                let buf: &[u8] = unsafe { core::mem::transmute(buf) };
                return Poll::Ready(Ok(buf));
            }

            //trace!("  empty");
            state.rx_waker.register(cx.waker());
            Poll::<Result<&[u8], embassy::io::Error>>::Pending
        })
    }
    fn consume(mut self: Pin<&mut Self>, amt: usize) {
        // info!("Consumed {} bytes", amt);
        self.inner.with(|state| {
            state.rx.pop(amt);
        });
        self.inner.pend();
    }
}

impl<'d, T: Instance> embassy::io::AsyncWrite for BufferedUart<'d, T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, embassy::io::Error>> {
        let poll = self.inner.with(|state| {
            let tx_buf = state.tx.push_buf();
            if tx_buf.is_empty() {
                //    trace!("poll_write: pending");
                state.tx_waker.register(cx.waker());
                return Poll::Pending;
            }

            let n = core::cmp::min(tx_buf.len(), buf.len());
            tx_buf[..n].copy_from_slice(&buf[..n]);
            state.tx.push(n);

            // Conservative compiler fence to prevent optimizations that do not
            // take in to account actions by DMA. The fence has been placed here,
            // before any DMA action has started
            compiler_fence(Ordering::SeqCst);

            Poll::Ready(Ok(n))
        });
        self.inner.pend();
        poll
    }
}
