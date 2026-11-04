use std::{cell::RefCell, rc::Rc};

use rbs_definition::collections::HashMap;

use crate::{
    subtyping::{relation::Relation, result::SubtypingResult},
    types::Type,
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
}
