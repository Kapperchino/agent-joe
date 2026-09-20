use crate::runtime::SessionRuntime;
use clients::response::RequestMode;
use commands::command::Command;
use tools::tool_defs::ToolEffect;
use turn_engine::machine::TurnMachine;

pub struct SessionChanges<'a> {
    pub runtime: &'a SessionRuntime,
}

enum ChangeOperation {
    Diff,
    Undo(String),
}

impl ChangeOperation {
    fn new(command: &Command, turn: &TurnMachine) -> anyhow::Result<Self> {
        match command {
            Command::Diff => Ok(Self::Diff),
            Command::Undo(_) if !turn.is_idle() => Err(anyhow::anyhow!(
                "Interrupt the active turn before undoing changes"
            )),
            Command::Undo(id) => Ok(Self::Undo(id.clone())),
            _ => Err(anyhow::anyhow!("Unsupported change command")),
        }
    }

    fn effect(&self) -> ToolEffect {
        match self {
            Self::Diff => ToolEffect::Read,
            Self::Undo(_) => ToolEffect::Write,
        }
    }
}

impl SessionChanges<'_> {
    pub async fn begin_turn(
        scope: &utils::execution::ExecutionScope,
        mode: RequestMode,
    ) -> anyhow::Result<()> {
        let changes = scope.changes.clone();
        match (scope.workspace(), mode) {
            (Ok(_), RequestMode::Continue | RequestMode::Compact) => {
                scope
                    .enter(utils::files::operation(move |workspace| {
                        changes.start(workspace)
                    }))
                    .await
            }
            _ => Ok(()),
        }
    }

    pub async fn run(&self, command: &Command, turn: &TurnMachine) -> anyhow::Result<String> {
        let operation = ChangeOperation::new(command, turn)?;
        let scope = self.runtime.scope.child();
        let effect = operation.effect();
        self.runtime.interaction.authorize(effect)?;
        let lease = self.runtime.workspace.acquire(effect, &scope).await?;
        let changes = scope.changes.clone();
        let result = scope
            .enter(utils::files::operation(move |workspace| match operation {
                ChangeOperation::Diff => changes.review(workspace).map(|review| review.render()),
                ChangeOperation::Undo(id) => changes
                    .undo(workspace, &id)
                    .map(|record| format!("Undid Joe edit {id}. Recorded reversal: {}", record.id)),
            }))
            .await;
        scope.finish().await;
        drop(lease);
        result
    }
}
