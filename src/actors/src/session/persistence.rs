use clients::failure::{Failure, FailureKind};

pub enum Persistence {
    Ready,
    Failed(Failure),
}

impl Persistence {
    pub(crate) fn committed<T>(&self, value: T) -> Result<T, Failure> {
        match self {
            Self::Ready => Ok(value),
            Self::Failed(failure) => Err(failure.clone()),
        }
    }

    pub(crate) fn fail(&mut self, error: anyhow::Error) -> Option<String> {
        match self {
            Self::Ready => {
                let message =
                    format!("Session storage failed: {error}. Automatic continuation stopped.");
                *self = Self::Failed(Failure::new(FailureKind::Tool, message.clone()));
                Some(message)
            }
            Self::Failed(_) => None,
        }
    }
}
