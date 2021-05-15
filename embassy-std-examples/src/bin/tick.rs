#![feature(min_type_alias_impl_trait)]
#![feature(impl_trait_in_bindings)]
#![feature(type_alias_impl_trait)]
#![feature(generic_associated_types)]

use core::cell::{Cell, UnsafeCell};
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::{Context, Poll};
use embassy::executor::{raw::Task, SpawnToken, Spawner};
use embassy::time::{Duration, Timer};
use embassy::util::Forever;
use embassy_std::Executor;
use log::*;

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
    New,
    Start(T::StartFuture<'a>),
    Process,
    Event(T::EventFuture<'a>),
}

struct RunFuture<'a, T: A + Unpin + 'static> {
    context: &'a B<'a, T>,
}

impl<'a, T: A + Unpin + 'a> Future for RunFuture<'a, T> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.context.poll(cx)
    }
}

struct B<'a, T: A + Unpin + 'static> {
    task: Task<RunFuture<'static, T>>,
    t: UnsafeCell<T>,
    state: Cell<ActorState<'a, T>>,
    counter: AtomicU32,
}

impl<'a, T: A + Unpin + 'static> B<'a, T> {
    fn mount(&'static mut self, spawner: Spawner) {
        let task = &self.task;
        let future = RunFuture { context: self };
        let token = Task::spawn(task, move || future);
        spawner.spawn(token).unwrap();
    }

    fn poll(&'a self, cx: &mut Context<'_>) -> Poll<()> {
        loop {
            match self.state.replace(ActorState::New) {
                ActorState::New => {
                    let fut = unsafe { &mut *self.t.get() }.do_start();
                    self.state.set(ActorState::Start(fut));
                }
                ActorState::Start(mut fut) => {
                    let r = unsafe { Pin::new_unchecked(&mut fut) }.poll(cx);
                    if r.is_pending() {
                        self.state.set(ActorState::Start(fut));
                        return Poll::Pending;
                    } else {
                        self.state.set(ActorState::Process);
                    }
                }
                ActorState::Process => {
                    let event = self.counter.fetch_add(1, Ordering::SeqCst);

                    let fut = unsafe { &mut *self.t.get() }.do_event(event);
                    self.state.set(ActorState::Event(fut));
                }
                ActorState::Event(mut fut) => {
                    let r = unsafe { Pin::new_unchecked(&mut fut) }.poll(cx);
                    if r.is_pending() {
                        self.state.set(ActorState::Event(fut));
                        return Poll::Pending;
                    } else {
                        self.state.set(ActorState::Process);
                    }
                }
            }
        }
    }
}

static EXECUTOR: Forever<Executor> = Forever::new();

fn main() {
    env_logger::builder()
        .filter_level(log::LevelFilter::Debug)
        .format_timestamp_nanos()
        .init();

    static mut R: B<C> = B {
        task: Task::new(),
        t: UnsafeCell::new(C {}),
        state: Cell::new(ActorState::New),
        counter: AtomicU32::new(0),
    };
    static mut U: B<C> = B {
        task: Task::new(),
        t: UnsafeCell::new(C {}),
        state: Cell::new(ActorState::New),
        counter: AtomicU32::new(10),
    };
    let executor = EXECUTOR.put(Executor::new());
    executor.run(|spawner| {
        unsafe { R.mount(spawner) };
        unsafe { U.mount(spawner) };
    });
}
