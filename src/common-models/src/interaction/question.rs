use super::{bounded_text, valid_id};
pub use commands::command::Answer;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuestionInput {
    pub id: String,
    pub prompt: String,
    pub required: bool,
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default = "allow_text")]
    pub allow_free_text: bool,
}

fn allow_text() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "QuestionInput")]
pub struct Question {
    pub id: String,
    pub prompt: String,
    pub required: bool,
    pub choices: Vec<Choice>,
    pub allow_free_text: bool,
}

impl TryFrom<QuestionInput> for Question {
    type Error = anyhow::Error;

    fn try_from(input: QuestionInput) -> anyhow::Result<Self> {
        let unique = input
            .choices
            .iter()
            .map(|choice| &choice.id)
            .collect::<BTreeSet<_>>();
        match valid_id(&input.id)
            && bounded_text(&input.prompt, 2048)
            && input.choices.len() <= 6
            && unique.len() == input.choices.len()
            && input
                .choices
                .iter()
                .all(|choice| valid_id(&choice.id) && bounded_text(&choice.label, 256))
            && (input.allow_free_text || !input.choices.is_empty())
        {
            true => Ok(Self {
                id: input.id,
                prompt: input.prompt,
                required: input.required,
                choices: input.choices,
                allow_free_text: input.allow_free_text,
            }),
            false => Err(anyhow::anyhow!(
                "Question needs a literal ID, a prompt of at most 2048 bytes, and at most six unique choices or free text"
            )),
        }
    }
}

impl Question {
    pub fn answer(&self, answer: &Answer) -> anyhow::Result<String> {
        match answer {
            Answer::Choice { choice_id } => self
                .choices
                .iter()
                .find(|choice| choice.id == *choice_id)
                .map(|choice| choice.label.clone())
                .ok_or_else(|| {
                    anyhow::anyhow!("Unknown choice {choice_id} for question {}", self.id)
                }),
            Answer::Text(text) if self.allow_free_text && bounded_text(text, 8192) => {
                Ok(text.clone())
            }
            Answer::Text(_) => Err(anyhow::anyhow!(
                "This question requires a listed choice or nonempty permitted text of at most 8192 bytes"
            )),
        }
    }

    pub fn display(&self) -> String {
        let kind = match self.required {
            true => "required",
            false => "optional",
        };
        let choices = self
            .choices
            .iter()
            .map(|choice| format!("{}: {}", choice.id, choice.label))
            .collect::<Vec<_>>()
            .join(" | ");
        let text = match self.allow_free_text {
            true => format!("\nFree text: /answer {} text <answer>.", self.id),
            false => String::new(),
        };
        let choices = match self.choices.is_empty() {
            true => String::new(),
            false => format!("\n{choices}\nChoose with /answer {} choice <id>.", self.id),
        };
        format!(
            "Question {} ({kind}): {}{choices}{text}",
            self.id, self.prompt
        )
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QuestionGate {
    #[default]
    Open,
    Required,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Questions {
    #[serde(default, rename = "questions")]
    pending: Vec<Question>,
    #[serde(default, rename = "answered_questions")]
    answered: BTreeSet<String>,
}

#[derive(Debug)]
pub struct AnsweredQuestion {
    pub id: String,
    pub prompt: String,
    pub text: String,
}

impl std::fmt::Display for AnsweredQuestion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Answer to question {} ({}): {}",
            self.id, self.prompt, self.text
        )
    }
}

impl Questions {
    pub fn pending(&self) -> &[Question] {
        &self.pending
    }

    pub fn gate(&self) -> QuestionGate {
        match self.pending.iter().any(|question| question.required) {
            true => QuestionGate::Required,
            false => QuestionGate::Open,
        }
    }

    pub fn ask(&mut self, question: Question) -> anyhow::Result<()> {
        match self.pending.len() < 8
            && self.pending.len() + self.answered.len() < 256
            && !self.pending.iter().any(|pending| pending.id == question.id)
            && !self.answered.contains(&question.id)
        {
            true => {
                self.pending.push(question);
                Ok(())
            }
            false => Err(anyhow::anyhow!(
                "Question IDs must be unused; at most eight questions may be pending and 256 may be answered per session"
            )),
        }
    }

    pub fn answer(&mut self, id: &str, answer: &Answer) -> anyhow::Result<AnsweredQuestion> {
        let question = self
            .pending
            .iter()
            .find(|question| question.id == id)
            .ok_or_else(|| anyhow::anyhow!("Question {id} is not pending"))?;
        let answered = AnsweredQuestion {
            id: id.into(),
            prompt: question.prompt.clone(),
            text: question.answer(answer)?,
        };
        self.pending.retain(|question| question.id != id);
        self.answered.insert(id.into());
        Ok(answered)
    }
}
