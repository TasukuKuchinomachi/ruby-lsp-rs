use crate::document::ParsedDocument;
use crate::global_state::GlobalState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SorbetLevel {
    Ignore,
    False,
    True,
    Strict,
    None,
}

impl SorbetLevel {
    pub fn from_sigil(sigil: Option<&str>) -> SorbetLevel {
        match sigil {
            Some("ignore") => SorbetLevel::Ignore,
            Some("false") => SorbetLevel::False,
            Some("true") => SorbetLevel::True,
            Some("strict") | Some("strong") => SorbetLevel::Strict,
            _ => SorbetLevel::None,
        }
    }

    pub fn for_document(global_state: &GlobalState, parsed: &ParsedDocument<'_>) -> SorbetLevel {
        if !parsed.document.is_ruby() || !global_state.has_type_checker {
            return SorbetLevel::Ignore;
        }
        SorbetLevel::from_sigil(parsed.magic_comment_value("typed").as_deref())
    }

    pub fn is_ignore(self) -> bool {
        self == SorbetLevel::Ignore
    }

    pub fn is_false(self) -> bool {
        self == SorbetLevel::False
    }

    pub fn is_true(self) -> bool {
        self == SorbetLevel::True
    }

    pub fn is_strict(self) -> bool {
        self == SorbetLevel::Strict
    }

    pub fn is_none(self) -> bool {
        self == SorbetLevel::None
    }

    pub fn is_true_or_higher(self) -> bool {
        matches!(self, SorbetLevel::True | SorbetLevel::Strict)
    }
}
