#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Starting,
    Recording,
    Saving,
    Converting,
    Failed,
}

pub struct Lifecycle {
    pub phase: Phase,
    pub close_requested: bool,
}
impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}
impl Lifecycle {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            close_requested: false,
        }
    }
    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Starting | Phase::Recording | Phase::Saving | Phase::Converting
        )
    }
    pub fn set_phase(&mut self, phase: Phase) {
        if self.phase == Phase::Saving && matches!(phase, Phase::Starting | Phase::Recording) {
            return;
        }
        self.phase = phase;
    }
    pub fn stop(&mut self) {
        if matches!(self.phase, Phase::Starting | Phase::Recording) {
            self.phase = Phase::Saving;
        }
    }
    pub fn request_close(&mut self) {
        self.close_requested = true;
        self.stop();
    }
    pub fn can_close(&self, worker_finished: bool) -> bool {
        self.close_requested && !self.busy() && worker_finished
    }
    pub fn finish(&mut self, success: bool) {
        self.phase = if success { Phase::Idle } else { Phase::Failed };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stopping_keeps_recording_busy_until_finalized() {
        let mut s = Lifecycle::new();
        s.set_phase(Phase::Recording);
        s.stop();
        assert!(s.busy());
        assert_eq!(s.phase, Phase::Saving);
    }
    #[test]
    fn closing_waits_for_saving_and_actual_worker_exit() {
        let mut s = Lifecycle::new();
        s.set_phase(Phase::Recording);
        s.request_close();
        assert!(!s.can_close(false));
        assert!(!s.can_close(true));
        s.finish(true);
        assert!(!s.can_close(false));
        assert!(s.can_close(true));
    }
    #[test]
    fn late_start_event_does_not_undo_a_stop() {
        let mut s = Lifecycle::new();
        s.set_phase(Phase::Starting);
        s.stop();
        s.set_phase(Phase::Recording);
        assert_eq!(s.phase, Phase::Saving);
    }
    #[test]
    fn failed_job_does_not_report_success() {
        let mut s = Lifecycle::new();
        s.set_phase(Phase::Converting);
        s.finish(false);
        assert_eq!(s.phase, Phase::Failed);
        assert!(!s.busy());
    }
}
