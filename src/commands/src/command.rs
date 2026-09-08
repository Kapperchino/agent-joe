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
        assert_eq!(Command::parse("diff"), Ok(Command::Diff));
        assert_eq!(
            Command::parse("undo joe-edit"),
            Ok(Command::Undo("joe-edit".into()))
        );
        assert!(Command::parse("undo").is_err());
        assert!(Command::parse("diff extra").is_err());
        assert_eq!(Command::parse("sessions"), Ok(Command::Sessions));
        assert_eq!(Command::parse("fork"), Ok(Command::Fork));
        assert_eq!(Command::parse("compact"), Ok(Command::Compact));
        assert!(Command::parse("fork workspace").is_err());
        assert!(Command::parse("compact extra").is_err());
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

    #[test]
    fn interaction_commands_preserve_text_and_require_explicit_answer_types() {
        assert_eq!(Command::parse("plan"), Ok(Command::Plan));
        assert_eq!(Command::parse("implement"), Ok(Command::Implement));
        assert_eq!(Command::parse("questions"), Ok(Command::Questions));
        assert_eq!(
            Command::parse("answer target choice lib"),
            Ok(Command::Answer(QuestionAnswer {
                id: "target".into(),
                answer: Answer::Choice {
                    choice_id: "lib".into()
                }
            }))
        );
        assert_eq!(
            Command::parse("answer target text keep  spacing\nand newlines"),
            Ok(Command::Answer(QuestionAnswer {
                id: "target".into(),
                answer: Answer::Text("keep  spacing\nand newlines".into())
            }))
        );
        assert_eq!(
            Command::parse("steer preserve  the API\nand tests"),
            Ok(Command::Steer("preserve  the API\nand tests".into()))
        );
        for input in [
            "plan extra",
            "implement extra",
            "answer",
            "answer target",
            "answer target lib",
            "answer target choice lib extra",
            "answer target text",
            "steer",
        ] {
            assert!(Command::parse(input).is_err(), "{input}");
        }
    }
}

#[derive(Debug, PartialEq, EnumString, VariantNames, Clone, EnumMessage)]
#[strum(serialize_all = "lowercase")]
pub enum Command {
    #[strum(message = "enters read-only planning mode; shows the current plan")]
    Plan,
    #[strum(message = "returns to implementation mode")]
    Implement,
    #[strum(
        message = "lists pending questions; /answer <id> choice <id> or /answer <id> text <answer>"
    )]
    Questions,
    #[strum(
        message = "answers a pending question; /answer <id> choice <id> or /answer <id> text <answer>"
    )]
    Answer(QuestionAnswer),
    #[strum(message = "corrects active work after cancellation and cleanup; /steer <correction>")]
    Steer(String),
    #[strum(message = "reviews the complete task diff, including staged and untracked changes")]
    Diff,
    #[strum(message = "undoes one recorded Joe edit; /undo <edit-id>")]
    Undo(String),
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
    #[strum(message = "forks this conversation in the same workspace")]
    Fork,
    #[strum(message = "compacts older context while preserving the saved transcript")]
    Compact,
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

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionAnswer {
    pub id: String,
    pub answer: Answer,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum Answer {
    Choice { choice_id: String },
    Text(String),
}

impl Default for Answer {
    fn default() -> Self {
        Self::Text(String::new())
    }
}

impl From<String> for Answer {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for Answer {
    fn from(text: &str) -> Self {
        Self::Text(text.into())
    }
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
            ["steer", _, ..] => Ok(Self::Steer(
                input
                    .trim()
                    .strip_prefix("steer")
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
            )),
            ["steer"] => Err("Use /steer <correction>".into()),
            ["answer", id, "choice", choice] => Ok(Self::Answer(QuestionAnswer {
                id: (*id).into(),
                answer: Answer::Choice {
                    choice_id: (*choice).into(),
                },
            })),
            ["answer", id, "text", _, ..] => {
                let answer = input
                    .trim()
                    .strip_prefix("answer")
                    .unwrap_or_default()
                    .trim_start()
                    .strip_prefix(id)
                    .unwrap_or_default()
                    .trim_start()
                    .strip_prefix("text")
                    .unwrap_or_default()
                    .trim_start();
                Ok(Self::Answer(QuestionAnswer {
                    id: (*id).into(),
                    answer: Answer::Text(answer.into()),
                }))
            }
            ["answer", ..] => {
                Err("Use /answer <id> choice <choice-id> or /answer <id> text <answer>".into())
            }
            ["undo", id] => Ok(Self::Undo((*id).to_owned())),
            ["undo"] => Err("Use /undo <recorded-edit-id>".into()),
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
