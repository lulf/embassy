#![feature(min_type_alias_impl_trait)]
#![feature(impl_trait_in_bindings)]
#![feature(type_alias_impl_trait)]

use embassy;
use embassy::executor::Spawner;
use embassy::time::{Duration, Timer};
use embassy::util::{AtomicWaker, Forever};
use embassy_std::Executor;

use channel::{consts, Channel};
use device::{Actor, ActorState, Address, Device};
use log::*;

struct MyActor {
    counter: u32,
}

impl MyActor {
    fn new() -> Self {
        Self { counter: 0 }
    }
}

impl Actor for MyActor {
    type Message = SayHello;
    fn handle(&mut self, channel: &Channel<'static, Self::Message, consts::U4>) {
        log::info!("Handle called!");
    }
}

pub struct SayHello;

// #[drogue::actor]

#[embassy::task]
async fn handle_myactor(
    actor: &'static MyActor,
    channel: &'static Channel<'static, SayHello, consts::U4>,
) {
    log::info!("Initializing");
    loop {
        log::info!("Awaiting request");
        let request = channel.receive().await;
        log::info!("Hello: {}", actor.counter);
    }
}

/*
#[embassy::task]
async fn handle_myactor(actor: &'static mut ActorState<'static, MyActor>) {
    log::info!("Handle: myactor");
}
*/

static EXECUTOR: Forever<Executor> = Forever::new();
static DEVICE: Forever<Device> = Forever::new();
static A1: Forever<ActorState<'static, MyActor>> = Forever::new();
static A2: Forever<ActorState<'static, MyActor>> = Forever::new();

// #[drogue::main]
fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .format_timestamp_nanos()
        .init();

    let mut executor = EXECUTOR.put(Executor::new());
    let mut device = DEVICE.put(Device::new());

    //   static a2 = A2.put(ActorState::new(MyActor::new()));

    //    let a1addr = device.register(a1);

    //device.mount(&mut a1, handle_foo());
    //    let a2addr = device.mount(&mut a2, handle_hello);

    //a1addr.send(SayHello);
    //   a2addr.send(SayHello);
    executor.run(|spawner| {
        #[embassy::task]
        async fn __actor_main(spawner: Spawner) {
            let a = A1.put(ActorState::new(MyActor::new()));
            a.channel.initialize();
            let a1addr = a.address();
            a1addr.send(SayHello).await;
            spawner.spawn(handle_myactor(&a.actor, &a.channel));
        }

        spawner.spawn(__actor_main(spawner)).unwrap();
    })
    //   device.run(executor);
}

mod device {
    use crate::channel::Channel;
    use embassy::executor::{SpawnToken, Spawner};
    use embassy_std::Executor;
    use heapless::{consts, Vec};

    pub struct Device<'a> {
        actors: Vec<&'a dyn RegisteredActor, consts::U8>,
    }

    pub struct ActorState<'a, A: Actor> {
        pub actor: A,
        pub channel: Channel<'a, A::Message, consts::U4>,
    }

    impl<'a, A: Actor> ActorState<'a, A> {
        pub fn new(actor: A) -> Self {
            let mut channel: Channel<'a, A::Message, consts::U4> = Channel::new();
            Self { actor, channel }
        }

        pub fn address(&'a self) -> Address<'a, A> {
            Address::new(&self.channel)
        }

        #[embassy::task]
        async fn handle() {
            //            state.actor.handle(&state.channel);
            log::info!("State async fn runnign!");
        }
    }

    trait RegisteredActor {
        fn spawn(&self, spawner: Spawner);
    }

    impl<'a, A: Actor> RegisteredActor for ActorState<'a, A> {
        fn spawn(&self, spawner: Spawner) {
            spawner.spawn(Self::handle());
        }
    }

    impl<'a> Device<'a> {
        pub fn new() -> Self {
            Self { actors: Vec::new() }
        }

        pub fn register<A: Actor>(&mut self, actor: &'a ActorState<A>) {
            let channel = &actor.channel;
            self.actors.push(actor);
        }

        pub fn run(&'a self, executor: &'static mut Executor) -> ! {
            executor.run(|spawner| {
                for a in self.actors.iter() {
                    a.spawn(spawner);
                }
            })
        }
    }

    pub trait Actor {
        type Message;
        fn handle(&mut self, channel: &Channel<'static, Self::Message, consts::U4>);
    }

    pub struct Address<'a, A: Actor> {
        channel: &'a Channel<'a, A::Message, consts::U4>,
    }

    impl<'a, A: Actor> Address<'a, A> {
        pub fn new(channel: &'a Channel<'a, A::Message, consts::U4>) -> Self {
            Self { channel }
        }
    }

    impl<'a, A: Actor> Address<'a, A> {
        pub async fn send(&self, message: A::Message) {
            self.channel.send(message).await
        }
    }

    impl<'a, A: Actor> Copy for Address<'a, A> {}

    impl<'a, A: Actor> Clone for Address<'a, A> {
        fn clone(&self) -> Self {
            Self {
                channel: self.channel,
            }
        }
    }
}

mod channel {

    use core::{
        cell::{RefCell, UnsafeCell},
        future::Future,
        pin::Pin,
        task::{Context, Poll},
    };
    use embassy::util::AtomicWaker;
    pub use heapless::consts;
    use heapless::{
        spsc::{Consumer, Producer, Queue},
        ArrayLength,
    };

    struct ChannelInner<'a, T, N: ArrayLength<T>> {
        queue: UnsafeCell<Queue<T, N>>,
        producer: RefCell<Option<Producer<'a, T, N>>>,
        consumer: RefCell<Option<Consumer<'a, T, N>>>,
        producer_waker: AtomicWaker,
        consumer_waker: AtomicWaker,
    }

    impl<'a, T, N: 'a + ArrayLength<T>> ChannelInner<'a, T, N> {
        pub fn new() -> Self {
            Self {
                queue: UnsafeCell::new(Queue::new()),
                producer: RefCell::new(None),
                consumer: RefCell::new(None),
                producer_waker: AtomicWaker::new(),
                consumer_waker: AtomicWaker::new(),
            }
        }

        fn split(&'a self) {
            let (producer, consumer) = unsafe { (&mut *self.queue.get()).split() };
            self.producer.borrow_mut().replace(producer);
            self.consumer.borrow_mut().replace(consumer);
        }

        fn poll_dequeue(&self, cx: &mut Context<'_>) -> Poll<T> {
            if let Some(value) = self.consumer.borrow_mut().as_mut().unwrap().dequeue() {
                self.producer_waker.wake();
                Poll::Ready(value)
            } else {
                self.consumer_waker.register(cx.waker());
                Poll::Pending
            }
        }

        fn poll_enqueue(&self, cx: &mut Context<'_>, element: &mut Option<T>) -> Poll<()> {
            let mut producer = self.producer.borrow_mut();
            if producer.as_mut().unwrap().ready() {
                let value = element.take().unwrap();
                producer.as_mut().unwrap().enqueue(value);
                self.consumer_waker.wake();
                Poll::Ready(())
            } else {
                self.producer_waker.register(cx.waker());
                Poll::Pending
            }
        }
    }

    pub struct Channel<'a, T, N: ArrayLength<T>> {
        inner: ChannelInner<'a, T, N>,
    }

    impl<'a, T, N: 'a + ArrayLength<T>> Channel<'a, T, N> {
        pub fn new() -> Self {
            let inner = ChannelInner::new();
            Self { inner }
        }

        pub fn initialize(&'a self) {
            self.inner.split();
        }

        pub fn send(&'a self, value: T) -> ChannelSend<'a, T, N> {
            ChannelSend {
                inner: &self.inner,
                element: Some(value),
            }
        }
        pub fn receive(&'a self) -> ChannelReceive<'a, T, N> {
            ChannelReceive { inner: &self.inner }
        }
    }

    pub struct ChannelSend<'a, T, N: ArrayLength<T>> {
        inner: &'a ChannelInner<'a, T, N>,
        element: Option<T>,
    }

    // TODO: Is this safe?
    impl<'a, T, N: ArrayLength<T>> Unpin for ChannelSend<'a, T, N> {}

    impl<'a, T, N: ArrayLength<T>> Future for ChannelSend<'a, T, N> {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.inner.poll_enqueue(cx, &mut self.element)
        }
    }

    pub struct ChannelReceive<'a, T, N: ArrayLength<T>> {
        inner: &'a ChannelInner<'a, T, N>,
    }

    impl<'a, T, N: ArrayLength<T>> Future for ChannelReceive<'a, T, N> {
        type Output = T;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.inner.poll_dequeue(cx)
        }
    }
}
