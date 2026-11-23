/// Quotes a value for safe use in a POSIX shell command.
#[inline]
pub fn shell_quote(value: &str) -> String { format!("'{}'", value.replace('\'', "'\\''")) }
