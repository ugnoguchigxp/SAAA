use std::process::Child;

pub(super) struct ProcessGuard {
    child: Child,
    terminated: bool,
}

impl ProcessGuard {
    pub(super) fn new(child: Child) -> Self {
        Self {
            child,
            terminated: false,
        }
    }

    pub(super) fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    pub(super) fn terminate(&mut self) {
        if self.terminated {
            return;
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.terminated = true;
    }
}

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.terminate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn terminate_is_idempotent_and_drop_reaps_the_child() {
        let child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep starts");
        let mut guard = ProcessGuard::new(child);
        let _ = guard.child_mut();
        guard.terminate();
        guard.terminate();
        drop(guard);
    }
}
