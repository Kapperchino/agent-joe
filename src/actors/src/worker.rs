use analysis::rust_context::Context;
use std::marker::PhantomData;

// Base unit for the agent, should be given context and then simply do the work
pub struct Worker<C: Context> {
    _ctx: PhantomData<C>,
}

impl<C: Context> Worker<C> {
    pub fn new() -> Self {
        Self { _ctx: PhantomData }
    }
}
