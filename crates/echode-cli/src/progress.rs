//! Advisory NDJSON on stdout; diagnostics and the recovered label stay on stderr.
use std::io::Write;

pub(crate) struct RenderProgress {
    enabled: bool,
    total: u32,
    rendered: u32,
    percent: u64,
}

impl RenderProgress {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled,
            total: 0,
            rendered: 0,
            percent: 0,
        }
    }

    pub(crate) fn start(&mut self, total: u32) {
        self.total = total;
        self.emit("rendering");
    }

    pub(crate) fn advance(&mut self, frames: u32) {
        self.rendered += frames;
        let percent = u64::from(self.rendered) * 100 / u64::from(self.total.max(1));
        if percent > self.percent {
            self.percent = percent;
            self.emit("rendering");
        }
    }

    pub(crate) fn finalizing(&mut self) {
        self.emit("finalizing");
    }

    pub(crate) fn complete(&mut self) {
        self.emit("complete");
    }

    fn emit(&mut self, phase: &str) {
        if !self.enabled {
            return;
        }
        // Called only outside render callbacks, at most once per integer percent.
        // Losing an observer must not turn an already published WAV into a failure.
        let mut output = std::io::stdout().lock();
        let result = writeln!(
            output,
            "{{\"version\":1,\"phase\":\"{phase}\",\"rendered_frames\":{},\"total_frames\":{}}}",
            self.rendered, self.total
        )
        .and_then(|()| output.flush());
        if result.is_err() {
            self.enabled = false;
        }
    }
}
