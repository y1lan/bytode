use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolTimeout(Duration);

impl ToolTimeout {
    pub const DEFAULT: Self = Self(Duration::from_millis(30_000));

    pub fn from_millis(timeout_ms: u64) -> Self {
        Self(Duration::from_millis(timeout_ms))
    }

    pub fn duration(self) -> Duration {
        self.0
    }
}

impl Default for ToolTimeout {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl From<ToolTimeout> for Duration {
    fn from(timeout: ToolTimeout) -> Self {
        timeout.duration()
    }
}

impl From<Duration> for ToolTimeout {
    fn from(timeout: Duration) -> Self {
        Self(timeout)
    }
}
