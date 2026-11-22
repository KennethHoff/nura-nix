//! How a run ends other than by finishing; `main` maps each variant to an exit code.
use std::fmt;

#[derive(Debug)]
pub enum Fail {
    Stop(String),
}

impl Fail {
    pub fn exit_code(&self) -> u8 {
        match self {
            Fail::Stop(_) => 1,
        }
    }
}

impl fmt::Display for Fail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Fail::Stop(m) = self;
        f.write_str(m)
    }
}

impl From<String> for Fail {
    fn from(s: String) -> Self {
        Fail::Stop(s)
    }
}

impl From<&str> for Fail {
    fn from(s: &str) -> Self {
        Fail::Stop(s.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Fail>;
