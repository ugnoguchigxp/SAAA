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
