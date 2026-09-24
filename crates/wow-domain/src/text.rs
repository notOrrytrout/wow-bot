use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NonEmptyText(String);

impl NonEmptyText {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        (!value.trim().is_empty()).then_some(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

pub fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf8_truncation_preserves_boundary() {
        let s = "ab😀cd";
        assert_eq!(truncate_utf8(s, 5), "ab");
        assert_eq!(truncate_utf8(s, 6), "ab😀");
    }
    #[test]
    fn identifier_grammar_is_stable() {
        assert!(valid_identifier("bot_1-a"));
        assert!(!valid_identifier("1bot"));
        assert!(!valid_identifier("bot space"));
    }
}
