use std::fmt;

#[derive(Debug)]
pub struct BackendFailure {
    pub context: String,
    pub code: i32,
    pub stderr: String,
}

impl BackendFailure {
    pub fn verbatim(&self) -> &str {
        self.stderr.trim_end_matches('\n')
    }
}

impl fmt::Display for BackendFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let stderr = self.verbatim();
        if stderr.is_empty() {
            write!(f, "{} exited with status {}", self.context, self.code)
        } else {
            f.write_str(stderr)
        }
    }
}

impl std::error::Error for BackendFailure {}
