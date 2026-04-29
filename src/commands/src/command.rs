use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};
use std::sync::Arc;
use strum::VariantNames;
use strum_macros::{EnumMessage, EnumString, VariantNames};

pub struct CommandContext {
    nucleo: Nucleo<String>,
}

#[derive(Debug, PartialEq, EnumString, VariantNames, Clone, EnumMessage)]
#[strum(serialize_all = "lowercase")]
pub enum Command {
    #[strum(serialize = "context")]
    #[strum(message = "prints out the context")]
    PrintContext,
    #[strum(message = "logs out the user")]
    Logout,
    #[strum(message = "clears the state")]
    Clear,
    #[strum(serialize = "model")]
    #[strum(message = "changes the model name effort")]
    ChangeModel(String, String),
}

impl CommandContext {
    pub fn new() -> CommandContext {
        // there will be no new matcher states
        let notify = Arc::new(|| {});
        let nucleo = Nucleo::<String>::new(Config::DEFAULT, notify, Some(1), 1);
        let injector = nucleo.injector();
        Command::print_all().into_iter().for_each(|x| {
            injector.push(x, |item, cols| {
                cols[0] = item.as_str().into();
            });
        });
        CommandContext { nucleo }
    }

    pub fn search(&mut self, string: &str) -> Vec<String> {
        self.nucleo
            .pattern
            .reparse(0, string, CaseMatching::Smart, Normalization::Smart, false);

        self.nucleo.tick(5);

        let snapshot = self.nucleo.snapshot();
        snapshot
            .matched_items(..)
            .into_iter()
            .map(|x| x.data.clone())
            .collect()
    }
}

impl Command {
    pub fn print_all() -> Vec<String> {
        Command::VARIANTS
            .into_iter()
            .map(|x: &&str| x.to_string())
            .collect()
    }
}
