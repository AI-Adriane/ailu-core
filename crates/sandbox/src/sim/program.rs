//! Simulated programs: Rust functions that stand in for a command.

use super::fault::SimFault;
use super::fs::SimFs;
use super::net::{NetDenied, SimConnection, SimNetwork};
use super::output::SimOutput;
use crate::limits::Limits;
use crate::request::NetworkPolicy;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use zeroize::Zeroizing;

/// What a simulated program returns: how it ended, or the fault it ran into.
pub type SimResult = Result<SimOutput, SimFault>;

/// A registered program. It is looked up by `argv[0]`, called with everything a real process
/// would see, and must be deterministic: the same invocation gives the same result.
pub type SimProgram = Arc<dyn Fn(&SimInvocation) -> SimResult + Send + Sync>;

/// Everything a simulated program can see and use: its arguments, its standard input, its
/// environment, its virtual filesystem, the simulated network under the sandbox's policy, and
/// the virtual time. It can reach nothing else.
///
/// `Debug` shows counts only: the environment holds the values of the secrets that were
/// granted.
pub struct SimInvocation {
    argv: Vec<String>,
    cwd: Option<String>,
    stdin: Vec<u8>,
    env: BTreeMap<String, Zeroizing<String>>,
    limits: Limits,
    fs: SimFs,
    policy: NetworkPolicy,
    net: Arc<SimNetwork>,
    now_ms: u64,
}

impl SimInvocation {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        argv: Vec<String>,
        cwd: Option<String>,
        stdin: Vec<u8>,
        env: BTreeMap<String, Zeroizing<String>>,
        limits: Limits,
        fs: SimFs,
        policy: NetworkPolicy,
        net: Arc<SimNetwork>,
        now_ms: u64,
    ) -> Self {
        Self {
            argv,
            cwd,
            stdin,
            env,
            limits,
            fs,
            policy,
            net,
            now_ms,
        }
    }

    /// The program followed by its arguments.
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    /// The arguments, without the program.
    pub fn args(&self) -> &[String] {
        &self.argv[1..]
    }

    /// The working directory relative to the root, `None` for the root itself.
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// Standard input.
    pub fn stdin(&self) -> &[u8] {
        &self.stdin
    }

    /// One environment variable: the fixed base set of the profile, or a granted secret.
    pub fn env(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(|value| value.as_str())
    }

    /// The names of the environment variables, sorted.
    pub fn env_names(&self) -> Vec<&str> {
        self.env.keys().map(String::as_str).collect()
    }

    /// The effective limits of the sandbox.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The virtual filesystem.
    pub fn fs(&self) -> &SimFs {
        &self.fs
    }

    /// The virtual time, in milliseconds.
    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// Resolve a name through the simulated network, under the sandbox's network policy.
    pub fn resolve(&self, host: &str) -> Result<Vec<IpAddr>, NetDenied> {
        self.net.resolve(&self.policy, host)
    }

    /// Connect to `host:port` through the simulated network, under the sandbox's network
    /// policy, following redirects.
    pub fn connect(&self, host: &str, port: u16) -> Result<SimConnection, NetDenied> {
        self.net.connect(&self.policy, host, port)
    }

    pub(crate) fn into_fs(self) -> SimFs {
        self.fs
    }
}

impl std::fmt::Debug for SimInvocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimInvocation")
            .field("argc", &self.argv.len())
            .field("stdin", &self.stdin.len())
            .field("env_vars", &self.env.len())
            .field("fs", &self.fs)
            .finish()
    }
}
