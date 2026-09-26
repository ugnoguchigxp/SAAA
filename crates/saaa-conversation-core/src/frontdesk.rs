use crate::contracts::{Action, MAX_PUBLIC_BYTES};

pub const DELEGATE_EXPLANATION: &str = "思考処理はまだ接続していません。";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    AwaitingControl,
    Streaming,
    Completed,
    Unsupported,
    Failed,
    Cancelled,
    Interrupted,
}

impl State {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Unsupported
                | Self::Failed
                | Self::Cancelled
                | Self::Interrupted
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidTransition,
    EmptyBody,
    TooLong,
}

/// I/O commits are performed by the host before it calls `adopt_control` or `append`.
/// The state machine never assumes that an uncommitted write may be published.
pub struct Frontdesk {
    state: State,
    action: Option<Action>,
    public_text: String,
}

impl Default for Frontdesk {
    fn default() -> Self {
        Self {
            state: State::AwaitingControl,
            action: None,
            public_text: String::new(),
        }
    }
}

impl Frontdesk {
    pub fn state(&self) -> State {
        self.state
    }

    pub fn action(&self) -> Option<&Action> {
        self.action.as_ref()
    }

    pub fn public_text(&self) -> &str {
        &self.public_text
    }

    pub fn adopt_control(&mut self, action: Action) -> Result<Option<&'static str>, Error> {
        if self.state != State::AwaitingControl {
            return Err(Error::InvalidTransition);
        }
        self.action = Some(action.clone());
        match action {
            Action::Reply | Action::Clarify => {
                self.state = State::Streaming;
                Ok(None)
            }
            Action::Delegate => {
                self.public_text.push_str(DELEGATE_EXPLANATION);
                self.state = State::Unsupported;
                Ok(Some(DELEGATE_EXPLANATION))
            }
        }
    }

    pub fn append(&mut self, text: &str) -> Result<(), Error> {
        if self.state != State::Streaming {
            return Err(Error::InvalidTransition);
        }
        if self.public_text.len() + text.len() > MAX_PUBLIC_BYTES {
            return Err(Error::TooLong);
        }
        self.public_text.push_str(text);
        Ok(())
    }

    pub fn complete(&mut self) -> Result<(), Error> {
        if self.state != State::Streaming {
            return Err(Error::InvalidTransition);
        }
        if self.public_text.trim().is_empty() {
            return Err(Error::EmptyBody);
        }
        self.state = State::Completed;
        Ok(())
    }

    pub fn fail(&mut self) {
        if !self.state.terminal() {
            self.state = State::Failed;
        }
    }

    pub fn cancel(&mut self) {
        if !self.state.terminal() {
            self.state = State::Cancelled;
        }
    }

    pub fn interrupt(&mut self) {
        if !self.state.terminal() {
            self.state = State::Interrupted;
        }
    }
}
