use std::time::Duration;

use assert_cmd::cmd::Command;

pub fn make_cli() -> Command {
    Command::cargo_bin("glaredb").expect("Failed to find binary")
}

#[allow(dead_code)] // Used in the tests. IDK why clippy is complaining about it.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(5_000);
