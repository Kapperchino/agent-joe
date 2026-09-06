use nucleo::pattern::{CaseMatching, Normalization};
use nucleo::{Config, Nucleo};
use std::sync::Arc;
use strum_macros::{EnumMessage, EnumString, VariantNames};

pub struct CommandContext {
    nucleo: Nucleo<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_commands_accept_only_their_expected_arguments() {
        assert_eq!(Command::parse("sessions"), Ok(Command::Sessions));
        assert_eq!(
            Command::parse("resume saved-session"),
            Ok(Command::Resume(ResumeTarget::Session {
                id: "saved-session".into()
            }))
        );
        assert_eq!(Command::parse(" new "), Ok(Command::New));
        assert_eq!(
            Command::parse("resume"),
            Ok(Command::Resume(ResumeTarget::Picker))
        );
        assert!(Command::parse("resume one two").is_err());
        assert!(Command::parse("clear extra").is_err());
        assert_eq!(Command::parse("context"), Ok(Command::PrintContext));
    }
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
    #[strum(message = "starts a new session, retaining previous sessions")]
    New,
    #[strum(message = "lists saved sessions in this project")]
    Sessions,
    #[strum(message = "opens the saved-session picker; /resume <id> resumes directly")]
    Resume(ResumeTarget),
    #[strum(serialize = "model")]
    #[strum(message = "changes the model name effort")]
    ChangeModel(String, String),
}

#[derive(Debug, Default, PartialEq, Clone)]
pub enum ResumeTarget {
    #[default]
    Picker,
    Session {
        id: String,
    },
}

impl CommandContext {
    pub fn new() -> CommandContext {
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
    pub fn parse(input: &str) -> Result<Self, String> {
        use std::str::FromStr;
        let words = input.split_whitespace().collect::<Vec<_>>();
        match words.as_slice() {
            ["resume", id] => Ok(Self::Resume(ResumeTarget::Session {
                id: (*id).to_owned(),
            })),
            ["resume"] => Ok(Self::Resume(ResumeTarget::Picker)),
            [name] => Self::from_str(name).map_err(|error| error.to_string()),
            _ => Err("Invalid command arguments".into()),
        }
    }

    pub fn print_all() -> Vec<String> {
        use strum::VariantNames;
        Command::VARIANTS
            .into_iter()
            .map(|x: &&str| x.to_string())
            .collect()
    }
}
