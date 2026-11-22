//! The two identifiers of a phone: the serial that names its directories and the slot it runs from.
use crate::fail::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Serial(String);

impl Serial {
    /// Safe as a directory name: not empty, not starting with `.` or `-`, only `[A-Za-z0-9._-]`.
    pub fn new(raw: &str) -> Result<Self> {
        let valid = !raw.is_empty()
            && !raw.starts_with(['.', '-'])
            && raw
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
        if valid {
            Ok(Serial(raw.to_string()))
        } else {
            Err(format!("the serial {raw:?} is not safe to use as a directory name").into())
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Serial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub fn as_str(self) -> &'static str {
        match self {
            Slot::A => "a",
            Slot::B => "b",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "a" => Some(Slot::A),
            "b" => Some(Slot::B),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serials_cannot_escape_the_phones_directory() {
        for bad in ["", ".", "..", "../x", ".hidden", "-rf", "a/b", "a b"] {
            assert!(Serial::new(bad).is_err(), "{bad:?}");
        }
        for good in ["ABC123", "a.b_c-d", "0"] {
            assert!(Serial::new(good).is_ok(), "{good:?}");
        }
    }
}
