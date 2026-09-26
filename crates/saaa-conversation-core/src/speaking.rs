use std::collections::VecDeque;

const MAX_CLAUSE_BYTES: usize = 2 * 1024;
const MAX_QUEUED_BYTES: usize = 8 * 1024;
const MAX_QUEUED_CLAUSES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Collecting,
    Synthesizing,
    Playing,
    Stopping,
    Played,
    Stopped,
    Failed,
    Interrupted,
}

impl State {
    pub fn terminal(self) -> bool {
        matches!(
            self,
            Self::Played | Self::Stopped | Self::Failed | Self::Interrupted
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clause {
    pub index: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidTransition,
    ClauseTooLong,
    QueueFull,
    EmptyPlayback,
}

/// One response owns one sequential speech lane. The host checkpoints each
/// returned clause before starting its TTS request.
pub struct Speaking {
    state: State,
    pending: String,
    queued: VecDeque<Clause>,
    queued_bytes: usize,
    current: Option<Clause>,
    next_index: u32,
    last_played: Option<u32>,
    body_finished: bool,
    generation: u64,
}

impl Default for Speaking {
    fn default() -> Self {
        Self {
            state: State::Idle,
            pending: String::new(),
            queued: VecDeque::new(),
            queued_bytes: 0,
            current: None,
            next_index: 0,
            last_played: None,
            body_finished: false,
            generation: 0,
        }
    }
}

impl Speaking {
    pub fn state(&self) -> State {
        self.state
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn last_played(&self) -> Option<u32> {
        self.last_played
    }

    pub fn start(&mut self) -> Result<(), Error> {
        if self.state != State::Idle {
            return Err(Error::InvalidTransition);
        }
        self.state = State::Collecting;
        Ok(())
    }

    pub fn push(&mut self, delta: &str) -> Result<Vec<Clause>, Error> {
        if !matches!(
            self.state,
            State::Collecting | State::Synthesizing | State::Playing
        ) || self.body_finished
        {
            return Err(Error::InvalidTransition);
        }
        let mut clauses = Vec::new();
        for ch in delta.chars() {
            self.pending.push(ch);
            if self.pending.len() > MAX_CLAUSE_BYTES {
                return Err(Error::ClauseTooLong);
            }
            if matches!(ch, '、' | '。' | '！' | '？' | ',' | '.' | '!' | '?') {
                if let Some(clause) = self.take_pending()? {
                    clauses.push(clause);
                }
            }
        }
        Ok(clauses)
    }

    pub fn finish_body(&mut self) -> Result<Option<Clause>, Error> {
        if !matches!(
            self.state,
            State::Collecting | State::Synthesizing | State::Playing
        ) || self.body_finished
        {
            return Err(Error::InvalidTransition);
        }
        self.body_finished = true;
        let final_clause = self.take_pending()?;
        self.maybe_complete();
        Ok(final_clause)
    }

    fn take_pending(&mut self) -> Result<Option<Clause>, Error> {
        if self.pending.trim().is_empty() {
            self.pending.clear();
            return Ok(None);
        }
        if self.queued.len() >= MAX_QUEUED_CLAUSES
            || self.queued_bytes + self.pending.len() > MAX_QUEUED_BYTES
        {
            return Err(Error::QueueFull);
        }
        let text = std::mem::take(&mut self.pending);
        let clause = Clause {
            index: self.next_index,
            text,
        };
        self.next_index += 1;
        self.queued_bytes += clause.text.len();
        self.queued.push_back(clause.clone());
        Ok(Some(clause))
    }

    pub fn begin_next(&mut self) -> Result<Option<Clause>, Error> {
        if !matches!(
            self.state,
            State::Collecting | State::Synthesizing | State::Playing
        ) || self.current.is_some()
        {
            return Err(Error::InvalidTransition);
        }
        let clause = self.queued.pop_front();
        if let Some(ref clause) = clause {
            self.queued_bytes -= clause.text.len();
            self.current = Some(clause.clone());
            self.state = State::Synthesizing;
        } else {
            self.maybe_complete();
        }
        Ok(clause)
    }

    pub fn playback_started(&mut self, index: u32, generation: u64) -> Result<(), Error> {
        if self.state != State::Synthesizing
            || self.generation != generation
            || self.current.as_ref().map(|c| c.index) != Some(index)
        {
            return Err(Error::InvalidTransition);
        }
        self.state = State::Playing;
        Ok(())
    }

    pub fn playback_ended(&mut self, index: u32, generation: u64) -> Result<(), Error> {
        if self.state != State::Playing
            || self.generation != generation
            || self.current.as_ref().map(|c| c.index) != Some(index)
        {
            return Err(Error::InvalidTransition);
        }
        self.last_played = Some(index);
        self.current = None;
        self.state = State::Collecting;
        self.maybe_complete();
        Ok(())
    }

    pub fn stop(&mut self) -> bool {
        if self.state.terminal() || self.state == State::Stopping {
            return false;
        }
        self.generation += 1;
        self.pending.clear();
        self.queued.clear();
        self.queued_bytes = 0;
        self.state = State::Stopping;
        true
    }

    pub fn confirm_stopped(&mut self) -> Result<(), Error> {
        if self.state != State::Stopping {
            return Err(Error::InvalidTransition);
        }
        self.current = None;
        self.state = State::Stopped;
        Ok(())
    }

    pub fn fail(&mut self) {
        if !self.state.terminal() {
            self.generation += 1;
            self.pending.clear();
            self.queued.clear();
            self.queued_bytes = 0;
            self.state = State::Failed;
        }
    }

    pub fn interrupt(&mut self) {
        if !self.state.terminal() {
            self.generation += 1;
            self.pending.clear();
            self.queued.clear();
            self.queued_bytes = 0;
            self.state = State::Interrupted;
        }
    }

    fn maybe_complete(&mut self) {
        if self.body_finished && self.queued.is_empty() && self.current.is_none() {
            self.state = State::Played;
        }
    }
}
