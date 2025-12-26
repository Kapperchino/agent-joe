use crate::actor::StreamAccu;
use crate::tools;
use crate::tools::{ListFilesInput, ReadFileInput, ToolResult};
use anyhow::Error;
use futures::future;

// Base unit for the agent, should be given context and then simply do the work
pub struct Worker {}
impl Worker {
}
