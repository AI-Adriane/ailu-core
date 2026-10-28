//! The machinery the cases share: how a case ends, a few executor-free future helpers, the tap
//! between the guard and the backend, and the environment of one case.

mod env;
mod poll;
mod tap;

pub(crate) use env::{Env, Lab, Source};
pub(crate) use poll::{poll_once, until, BoxFut};
pub(crate) use tap::Tap;

use crate::error::SandboxError;

/// Why a case stopped before its end.
pub(crate) enum Stop {
    /// The target does not meet the obligation.
    Fail(String),
    /// The target cannot provoke or observe what the case needs.
    Skip(String),
    /// The case ran to its end and measured something it cannot judge.
    Measured(String),
}

/// How a case ends: `Ok` is a pass.
pub(crate) type Verdict = Result<(), Stop>;

/// Stop the case with a failure unless the condition holds. The message is a fixed text with
/// codes: it must never hold content of the run.
macro_rules! ensure {
    ($cond:expr, $($msg:tt)+) => {
        if !($cond) {
            return Err($crate::conformance::support::Stop::Fail(format!($($msg)+)));
        }
    };
}

/// Stop the case with a failure.
macro_rules! fail {
    ($($msg:tt)+) => {
        return Err($crate::conformance::support::Stop::Fail(format!($($msg)+)))
    };
}

/// Stop the case: the target cannot take part in it.
macro_rules! skip {
    ($($msg:tt)+) => {
        return Err($crate::conformance::support::Stop::Skip(format!($($msg)+)))
    };
}

pub(crate) use {ensure, fail, skip};

/// What a result looks like in a report: `ok`, or the full error code. Never the message.
pub(crate) fn shape<T>(result: &Result<T, SandboxError>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(error) => error.full_code(),
    }
}

/// Fail unless `result` is the error with the full code `code`.
pub(crate) fn expect_code<T>(result: &Result<T, SandboxError>, code: &str, what: &str) -> Verdict {
    ensure!(
        matches!(result, Err(error) if error.full_code() == code),
        "{what}: expected {code}, got {}",
        shape(result)
    );
    Ok(())
}

/// Fail unless `result` is an error with one of the full codes.
pub(crate) fn expect_one_of<T>(
    result: &Result<T, SandboxError>,
    codes: &[&str],
    what: &str,
) -> Verdict {
    ensure!(
        matches!(result, Err(error) if codes.contains(&error.full_code())),
        "{what}: expected one of {codes:?}, got {}",
        shape(result)
    );
    Ok(())
}

/// Turn an unexpected error in a set-up step into a failure that names only its code.
pub(crate) trait Must<T> {
    fn must(self, what: &str) -> Result<T, Stop>;
}

impl<T> Must<T> for Result<T, SandboxError> {
    fn must(self, what: &str) -> Result<T, Stop> {
        self.map_err(|error| Stop::Fail(format!("{what}: refused with {}", error.full_code())))
    }
}
