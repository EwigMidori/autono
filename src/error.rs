use std::error::Error as StdError;

#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),
    #[error("{context}: {source}")]
    Context {
        context: String,
        #[source]
        source: Box<dyn StdError + Send + Sync>,
    },
    #[error(transparent)]
    Env(#[from] std::env::VarError),
    #[error(transparent)]
    Header(#[from] reqwest::header::InvalidHeaderValue),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    #[error(transparent)]
    TimeFormat(#[from] time::error::Format),
    #[error(transparent)]
    TimeParse(#[from] time::error::Parse),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
    #[error(transparent)]
    Utf8(#[from] std::string::FromUtf8Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

pub(crate) trait ResultContext<T> {
    fn context(self, context: impl Into<String>) -> Result<T>;
    fn with_context(self, context: impl FnOnce() -> String) -> Result<T>;
}

impl<T, E> ResultContext<T> for std::result::Result<T, E>
where
    E: StdError + Send + Sync + 'static,
{
    fn context(self, context: impl Into<String>) -> Result<T> {
        self.map_err(|source| Error::Context {
            context: context.into(),
            source: Box::new(source),
        })
    }

    fn with_context(self, context: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|source| Error::Context {
            context: context(),
            source: Box::new(source),
        })
    }
}

pub(crate) trait OptionContext<T> {
    fn context(self, context: impl Into<String>) -> Result<T>;
    fn with_context(self, context: impl FnOnce() -> String) -> Result<T>;
}

impl<T> OptionContext<T> for Option<T> {
    fn context(self, context: impl Into<String>) -> Result<T> {
        self.ok_or_else(|| Error::message(context.into()))
    }

    fn with_context(self, context: impl FnOnce() -> String) -> Result<T> {
        self.ok_or_else(|| Error::message(context()))
    }
}
