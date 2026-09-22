#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
use macos::Assertion;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleSleep {
    Allowed,
    Prevented,
}

pub struct SleepInhibitor {
    name: String,
    state: InhibitorState,
}

enum InhibitorState {
    Idle,
    Active { _assertion: Assertion },
    Unavailable,
}

impl SleepInhibitor {
    pub fn new(name: String) -> Self {
        Self {
            name,
            state: InhibitorState::Idle,
        }
    }

    pub fn update(&mut self, idle_sleep: IdleSleep) -> anyhow::Result<()> {
        match (&self.state, idle_sleep) {
            (_, IdleSleep::Allowed) => {
                self.state = InhibitorState::Idle;
                Ok(())
            }
            (InhibitorState::Idle, IdleSleep::Prevented) => {
                self.state = InhibitorState::Unavailable;
                self.state = InhibitorState::Active {
                    _assertion: Assertion::new(&self.name)?,
                };
                Ok(())
            }
            (InhibitorState::Active { .. } | InhibitorState::Unavailable, IdleSleep::Prevented) => {
                Ok(())
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
struct Assertion;

#[cfg(not(target_os = "macos"))]
impl Assertion {
    fn new(_: &str) -> anyhow::Result<Self> {
        Ok(Self)
    }
}

#[cfg(all(test, target_os = "macos"))]
#[path = "../tests/unit/power/tests.rs"]
mod tests;
