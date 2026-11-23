use snafu::Snafu;

#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum Error {
    #[snafu(display("HOME environment variable is not set"))]
    HomeNotFound,

    #[snafu(display("Could not write to stdout: {source}"))]
    WriteStdout { source: std::io::Error },
}

impl From<Error> for crate::error::Error {
    fn from(value: Error) -> Self {
        match value {
            Error::HomeNotFound => Self::HomeNotFound,
            Error::WriteStdout { source } => Self::WriteStdout { source },
        }
    }
}
