use analysis::rust_context::Context;
use ractor::{Actor, ActorCell};
use std::marker::PhantomData;

pub trait Worker: Actor {
    type Context: Context;
    fn get_background_actors(&self) -> Vec<ActorParams<Self>>;
}

pub struct BaseWorker<C: Context> {
    _ctx: PhantomData<C>,
}

impl<C: Context> Worker for BaseWorker<C>
where
    BaseWorker<C>: Actor,
{
    type Context = C;

    fn get_background_actors(&self) -> Vec<ActorParams<Self>> {
        todo!()
    }
}

impl<C: Context> BaseWorker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}

pub struct ActorParams<A: Actor> {
    name: Option<String>,
    handler: A,
    dep: A::Arguments,
    supervisor: ActorCell,
}
