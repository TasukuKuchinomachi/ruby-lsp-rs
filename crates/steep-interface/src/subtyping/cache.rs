use std::{cell::RefCell, rc::Rc};

use rbs_definition::collections::HashMap;

use crate::{
    subtyping::{relation::Relation, result::SubtypingResult},
    types::{Type, is_fresh_variable_name},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub relation: Relation<Type>,
    pub self_type: Option<Type>,
    pub instance_type: Option<Type>,
    pub class_type: Option<Type>,
    pub bounds: Vec<(String, Type)>,
}

#[derive(Default)]
pub struct Cache {
    subtypes: RefCell<HashMap<CacheKey, Rc<SubtypingResult>>>,
}

impl Cache {
    pub fn get(&self, key: &CacheKey) -> Option<Rc<SubtypingResult>> {
        self.subtypes.borrow().get(key).cloned()
    }

    pub fn insert(&self, key: CacheKey, result: Rc<SubtypingResult>) {
        self.subtypes.borrow_mut().insert(key, result);
    }

    pub fn forget_fresh_variables(&self) {
        self.subtypes
            .borrow_mut()
            .retain(|key, _| !key.mentions_fresh_variable());
    }
}

impl CacheKey {
    fn mentions_fresh_variable(&self) -> bool {
        [&self.relation.sub_type, &self.relation.super_type]
            .into_iter()
            .chain(&self.self_type)
            .chain(&self.instance_type)
            .chain(&self.class_type)
            .chain(self.bounds.iter().map(|(_, bound)| bound))
            .any(Type::mentions_fresh_variable)
            || self
                .bounds
                .iter()
                .any(|(name, _)| is_fresh_variable_name(name))
    }
}
