use crate::{source::Source, type_construction::Checker};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum InferenceRule {
    CollectionLiteralWithHintOfSelfReturningCall,
}

impl InferenceRule {
    pub const ALL: [InferenceRule; 1] =
        [InferenceRule::CollectionLiteralWithHintOfSelfReturningCall];

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InferenceRules {
    enabled: u8,
}

impl InferenceRules {
    pub const STEEP: InferenceRules = InferenceRules { enabled: 0 };

    pub fn more_than_steep() -> InferenceRules {
        InferenceRule::ALL
            .into_iter()
            .fold(InferenceRules::STEEP, InferenceRules::with)
    }

    pub fn only(rule: InferenceRule) -> InferenceRules {
        InferenceRules::STEEP.with(rule)
    }

    pub fn with(self, rule: InferenceRule) -> InferenceRules {
        InferenceRules {
            enabled: self.enabled | rule.bit(),
        }
    }

    pub fn without(self, rule: InferenceRule) -> InferenceRules {
        InferenceRules {
            enabled: self.enabled & !rule.bit(),
        }
    }

    pub fn contains(self, rule: InferenceRule) -> bool {
        self.enabled & rule.bit() != 0
    }
}

pub struct Inference {
    pub rules: InferenceRules,
}

impl Inference {
    pub fn steep() -> Inference {
        Inference {
            rules: InferenceRules::STEEP,
        }
    }

    pub fn prepare(
        _checker: &Checker,
        rules: InferenceRules,
        source: Source,
    ) -> (Source, Inference) {
        (source, Inference { rules })
    }
}
