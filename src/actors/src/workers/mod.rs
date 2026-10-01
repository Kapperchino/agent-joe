pub mod base_worker;
pub mod compaction_worker;
pub mod knowledge_worker;
pub mod read_worker;
pub mod simple_worker;
pub mod snapshot_worker;
pub mod task_worker;
pub mod validate_worker;
pub mod write_worker;

fn context_prompt(prompt: &str) -> String {
    format!("{prompt}\n\n{}", include_str!("resources/knowledge.md"))
}
