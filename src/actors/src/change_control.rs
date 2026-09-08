use crate::actor_state::ActorState;
use analysis::contexts::context::Context;
use commands::command::Command;
use common_models::tui_models::ActorToTuiPacket;
use tools::tool_defs::ToolEffect;

impl<C: Context + Clone + 'static> ActorState<C> {
    pub(crate) async fn change_command(&mut self, command: Command) {
        let result = self.run_change_command(&command).await;
        self.reporter.send(ActorToTuiPacket::CommandResult(
            command,
            result.unwrap_or_else(|error| format!("Change operation failed: {error:#}")),
        ));
    }

    async fn run_change_command(&self, command: &Command) -> anyhow::Result<String> {
        let runtime = &self.dependency.runtime;
        let scope = runtime.scope.child();
        let effect = match command {
            Command::Undo(_) if !self.turn.is_idle() => Err(anyhow::anyhow!(
                "Interrupt the active turn before undoing changes"
            )),
            Command::Undo(_) => Ok(ToolEffect::Write),
            _ => Ok(ToolEffect::Read),
        }?;
        let lease = runtime.workspace.acquire(effect, &scope).await?;
        let changes = scope.changes.clone();
        let command = command.clone();
        let result = scope
            .enter(utils::files::operation(move |workspace| match command {
                Command::Diff => changes.review(workspace).map(|review| review.render()),
                Command::Undo(id) => changes
                    .undo(workspace, &id)
                    .map(|record| format!("Undid Joe edit {id}. Recorded reversal: {}", record.id)),
                _ => Err(anyhow::anyhow!("Unsupported change command")),
            }))
            .await;
        scope.finish().await;
        drop(lease);
        result
    }
}
