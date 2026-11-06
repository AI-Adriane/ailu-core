//! What a simulated program produces.

use crate::outcome::DeclaredExit;

/// The most bytes the simulator ever materializes for one stream, whatever the limit asks.
/// It keeps the memory of a test bounded even when a limit is set absurdly high.
pub const MAX_STREAM_BYTES: u64 = 8 * 1024 * 1024;

/// One output stream of a simulated program.
///
/// A program that "writes a terabyte" does not allocate one: it returns
/// [`SimStream::Repeated`], which only *describes* the bytes, and the simulator materializes
/// no more than the output bound.
///
/// `Debug` shows the length, never the bytes.
#[derive(Clone, PartialEq, Eq)]
pub enum SimStream {
    /// Literal bytes.
    Bytes(Vec<u8>),
    /// `len` copies of `byte`, not stored.
    Repeated {
        /// The repeated byte.
        byte: u8,
        /// How many times.
        len: u64,
    },
}

impl SimStream {
    /// An empty stream.
    pub fn empty() -> Self {
        SimStream::Bytes(Vec::new())
    }

    /// Length in bytes, as the program meant to write it.
    pub fn len(&self) -> u64 {
        match self {
            SimStream::Bytes(bytes) => bytes.len() as u64,
            SimStream::Repeated { len, .. } => *len,
        }
    }

    /// Whether the stream is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The first `min(len, cap, MAX_STREAM_BYTES)` bytes, and whether anything was cut.
    pub fn take(&self, cap: u64) -> (Vec<u8>, bool) {
        let keep = self.len().min(cap).min(MAX_STREAM_BYTES);
        let cut = keep < self.len();
        // `keep` is at most `MAX_STREAM_BYTES`, so the conversion cannot fail.
        let keep = usize::try_from(keep).unwrap_or(0);
        let bytes = match self {
            SimStream::Bytes(bytes) => bytes[..keep].to_vec(),
            SimStream::Repeated { byte, .. } => vec![*byte; keep],
        };
        (bytes, cut)
    }
}

impl From<&str> for SimStream {
    fn from(value: &str) -> Self {
        SimStream::Bytes(value.as_bytes().to_vec())
    }
}

impl From<String> for SimStream {
    fn from(value: String) -> Self {
        SimStream::Bytes(value.into_bytes())
    }
}

impl From<Vec<u8>> for SimStream {
    fn from(value: Vec<u8>) -> Self {
        SimStream::Bytes(value)
    }
}

impl From<&[u8]> for SimStream {
    fn from(value: &[u8]) -> Self {
        SimStream::Bytes(value.to_vec())
    }
}

impl std::fmt::Debug for SimStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SimStream({} bytes)", self.len())
    }
}

/// What a simulated program says it consumed. The simulator compares it with the limits of the
/// sandbox and kills the program (`LimitExceeded`) when one is over.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SimUsage {
    /// Virtual running time. Compared with `wall_time_ms`.
    pub duration_ms: u64,
    /// Peak memory. Compared with `memory_bytes`.
    pub memory_bytes: u64,
    /// Peak number of processes. Compared with `max_processes`.
    pub processes: u32,
    /// Disk used on top of what the program wrote to its filesystem (which the simulator
    /// counts itself). Compared with `ephemeral_disk_bytes`.
    pub disk_bytes: u64,
}

/// The result of a simulated program that ran to its end.
///
/// `Debug` shows lengths and the kind of exit, never the bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct SimOutput {
    /// How the program ended: an exit code or a signal.
    pub exit: DeclaredExit,
    /// Standard output.
    pub stdout: SimStream,
    /// Standard error.
    pub stderr: SimStream,
    /// What the program consumed.
    pub usage: SimUsage,
}

impl SimOutput {
    /// Exit code 0, nothing written, nothing consumed.
    pub fn success() -> Self {
        Self {
            exit: DeclaredExit::Code { code: 0 },
            stdout: SimStream::empty(),
            stderr: SimStream::empty(),
            usage: SimUsage::default(),
        }
    }

    /// Set standard output.
    pub fn with_stdout(mut self, stdout: impl Into<SimStream>) -> Self {
        self.stdout = stdout.into();
        self
    }

    /// Set standard error.
    pub fn with_stderr(mut self, stderr: impl Into<SimStream>) -> Self {
        self.stderr = stderr.into();
        self
    }

    /// Set how the program ended.
    pub fn with_exit(mut self, exit: DeclaredExit) -> Self {
        self.exit = exit;
        self
    }

    /// Set the exit code.
    pub fn with_exit_code(self, code: i32) -> Self {
        self.with_exit(DeclaredExit::Code { code })
    }

    /// Set what the program consumed.
    pub fn with_usage(mut self, usage: SimUsage) -> Self {
        self.usage = usage;
        self
    }
}

impl std::fmt::Debug for SimOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimOutput")
            .field("stdout", &self.stdout)
            .field("stderr", &self.stderr)
            .field("usage", &self.usage)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_cuts_at_the_cap_without_allocating_the_whole_stream() {
        let huge = SimStream::Repeated {
            byte: b'x',
            len: 1 << 40,
        };
        let (bytes, cut) = huge.take(10);
        assert_eq!(bytes, vec![b'x'; 10]);
        assert!(cut);
        let (bytes, cut) = huge.take(u64::MAX);
        assert_eq!(bytes.len() as u64, MAX_STREAM_BYTES, "hard cap");
        assert!(cut);
        let (bytes, cut) = SimStream::from("abc").take(3);
        assert_eq!(bytes, b"abc");
        assert!(!cut);
        let (bytes, cut) = SimStream::from("abcd").take(3);
        assert_eq!(bytes, b"abc");
        assert!(cut);
    }

    #[test]
    fn debug_shows_lengths_not_bytes() {
        let out = SimOutput::success()
            .with_stdout("s3cr3t-out")
            .with_stderr("s3cr3t-err");
        let shown = format!("{out:?}");
        assert!(!shown.contains("s3cr3t"), "{shown}");
        assert!(shown.contains("10 bytes"), "{shown}");
    }
}
