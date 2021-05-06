#![feature(min_type_alias_impl_trait)]
#![feature(impl_trait_in_bindings)]
#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use embassy::executor::{raw::Task, SpawnToken, Spawner};
use embassy::time::{Duration, Timer};
use embassy::util::Forever;
use embassy_std::Executor;
use log::*;

struct RunnableFuture {
    actor: &'static mut dyn Runnable,
}

impl Future for RunnableFuture {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.actor.poll(cx)
    }
}

trait Runnable {
    fn run(&'static mut self) -> RunnableFuture;
    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()>;
    fn spawn(&'static mut self, spawner: Spawner);
}

trait A {
    type StartFuture<'m>: Future<Output = ()> + 'm
    where
        Self: 'm;
    fn do_start<'m>(&'m mut self) -> Self::StartFuture<'m>;

    type EventFuture<'m>: Future<Output = ()> + 'm;
    fn do_event<'m>(&mut self, event: u32) -> Self::EventFuture<'m>;
}

struct C;

impl A for C {
    type StartFuture<'m> = impl Future<Output = ()> + 'm;
    fn do_start<'m>(&'m mut self) -> Self::StartFuture<'m> {
        async move {
            println!("Started!");
        }
    }

    type EventFuture<'m> = impl Future<Output = ()> + 'm;
    fn do_event<'m>(&mut self, event: u32) -> Self::EventFuture<'m> {
        async move {
            println!("Tick {}", event);
            Timer::after(Duration::from_secs(1)).await;
        }
    }
}

enum ActorState<'a, T: A + 'a> {
    Start(T::StartFuture<'a>),
    Process,
    Event(T::EventFuture<'a>),
}

struct B<'a, T: A + 'a> {
    task: Task<impl Future + 'a>,
    state: Option<ActorState<'a, T>>,
    counter: u32,
    t: T,
}

impl<'a, T: A + Unpin> B<'a, T> {
    pub fn initialize(&'a mut self) {
        let fut = self.t.do_start();
        self.state.replace(ActorState::Start(fut)); //unsafe { core::mem::transmute_copy(&fut) });
    }
}

impl<'a, T: A + Unpin> Runnable for B<'a, T> {
    fn run(&'static mut self) -> RunnableFuture {
        RunnableFuture { actor: self }
    }

    fn spawn(&'static mut self, spawner: Spawner) {

        /*
        -> SpawnToken<impl Future + 'static> {
                    type T = impl Future + 'static;
                    static TASK: Task<T> = Task::new();
                    Task::spawn(&TASK, move || async move {
                        runnable.run().await;
                    })
                }
                */
    }

    fn poll(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let mut counter = 0;
        loop {
            match self.state.take().unwrap() {
                ActorState::Start(mut fut) => {
                    let r = unsafe { Pin::new_unchecked(&mut fut) }.poll(cx);
                    if r.is_pending() {
                        self.state.replace(ActorState::Start(fut));
                        return Poll::Pending;
                    } else {
                        self.state.replace(ActorState::Process);
                    }
                }
                ActorState::Process => {
                    let event = self.counter;
                    self.counter += 1;

                    let fut = self.t.do_event(event);
                    self.state.replace(ActorState::Event(fut));
                }
                ActorState::Event(mut fut) => {
                    let r = unsafe { Pin::new_unchecked(&mut fut) }.poll(cx);
                    if r.is_pending() {
                        self.state.replace(ActorState::Event(fut));
                        return Poll::Pending;
                    } else {
                        self.state.replace(ActorState::Process);
                    }
                }
            }
        }
    }
}

fn run(runnable: &'static mut dyn Runnable) -> SpawnToken<impl Future + 'static> {
    type T = impl Future + 'static;
    static TASK: Task<T> = Task::new();
    Task::spawn(&TASK, move || async move {
        runnable.run().await;
    })
}

static EXECUTOR: Forever<Executor> = Forever::new();

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .format_timestamp_nanos()
        .init();

    static mut R: B<C> = B {
        task: Task::new(),
        t: C {},
        state: None,
        counter: 0,
    };
    unsafe { R.initialize() };

    static mut U: B<C> = B {
        task: Task::new(),
        t: C {},
        state: None,
        counter: 10,
    };
    unsafe { U.initialize() };
    let executor = EXECUTOR.put(Executor::new());
    executor.run(|spawner| {
        spawner.spawn(run(unsafe { &mut R })).unwrap();
        spawner.spawn(run(unsafe { &mut U })).unwrap();
    });
}
