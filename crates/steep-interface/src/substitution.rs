use rbs_definition::collections::HashMap;

use crate::types::Type;

#[derive(Clone, Debug, Default)]
pub struct Substitution {
    pub dictionary: HashMap<String, Type>,
    pub instance_type: Option<Type>,
    pub module_type: Option<Type>,
    pub self_type: Option<Type>,
}

impl Substitution {
    pub fn empty() -> Substitution {
        Substitution {
            dictionary: HashMap::default(),
            instance_type: Some(Type::Instance),
            module_type: Some(Type::Class),
            self_type: Some(Type::SelfType),
        }
    }

    pub fn build(
        variables: &[String],
        types: &[Type],
        instance_type: Option<Type>,
        module_type: Option<Type>,
        self_type: Option<Type>,
    ) -> Substitution {
        assert_eq!(variables.len(), types.len(), "invalid substitution");
        Substitution {
            dictionary: variables
                .iter()
                .cloned()
                .zip(types.iter().cloned())
                .collect(),
            instance_type,
            module_type,
            self_type,
        }
    }

    pub fn except(&self, variables: &[String]) -> Substitution {
        let mut substitution = self.clone();
        for variable in variables {
            substitution.dictionary.remove(variable);
        }
        substitution
    }

    pub fn with_self_type(&self, self_type: Option<Type>) -> Substitution {
        Substitution {
            self_type,
            ..self.clone()
        }
    }

    pub fn merge(&self, other: &Substitution, overwrite: bool) -> Substitution {
        let mut dictionary: HashMap<String, Type> = self
            .dictionary
            .iter()
            .map(|(variable, type_)| (variable.clone(), type_.substitute(other)))
            .collect();
        for (variable, type_) in &other.dictionary {
            match dictionary.get(variable) {
                Some(existing) if existing == type_ => {}
                Some(existing) if !overwrite => {
                    panic!("duplicated key on merge: {variable}, {existing}, {type_}")
                }
                _ => {
                    dictionary.insert(variable.clone(), type_.clone());
                }
            }
        }
        Substitution {
            dictionary,
            instance_type: self
                .instance_type
                .as_ref()
                .map(|type_| type_.substitute(other)),
            module_type: self
                .module_type
                .as_ref()
                .map(|type_| type_.substitute(other)),
            self_type: self.self_type.as_ref().map(|type_| type_.substitute(other)),
        }
    }
}
