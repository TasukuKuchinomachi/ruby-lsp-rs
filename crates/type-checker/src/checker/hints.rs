use rbs_environment::{Literal, RecordField, Type};
use ruby_prism::Node;

use super::{Checker, Context, Locals};
use crate::source::{Span, bytes_text};
use crate::steep_relation;
use crate::type_utilities::{array_of, element_type, hash_of, is_bottom, members_of, remove_nil};
use crate::{Severity, SteepVersion};

impl<'pr> Checker<'_, 'pr> {
    /// Steep's `unwrap`: the hint without `nil`, an alias expanded once
    pub(crate) fn unwrap_hint(&self, hint: &Type) -> Type {
        let expanded = match hint {
            Type::Alias { name, arguments } => self.environment.expand_alias(name, arguments).unwrap_or(hint.clone()),
            other => other.clone(),
        };
        remove_nil(&expanded)
    }

    /// Steep's `test_literal_type`: a literal typed with a hint it fits takes the hint itself
    pub(crate) fn literal_type_from_hint(&self, literal: Literal, hint: Option<&Type>) -> Option<Type> {
        let hint = hint.filter(|hint| !hint.is_untyped())?;
        steep_relation::is_subtype(self.environment, &Type::Literal(literal), hint).then(|| self.unwrap_hint(hint))
    }

    /// Steep types `true` and `false` as `bool`, or as the hint when they fit a hint that is neither `untyped` nor
    /// `top`
    pub(crate) fn boolean_literal_type(&self, literal: Literal, hint: Option<&Type>) -> Type {
        match hint {
            Some(hint) if !matches!(hint, Type::Untyped | Type::Top) => self
                .literal_type_from_hint(literal, Some(hint))
                .unwrap_or(Type::Bool),
            _ => Type::Bool,
        }
    }

    /// Steep's `typing.new_child`: what `inference` records and assigns is kept only when `accept` holds of its
    /// result, which is returned either way
    pub(crate) fn try_inference<T>(
        &mut self,
        locals: &mut Locals,
        inference: impl FnOnce(&mut Self, &mut Locals) -> T,
        accept: impl FnOnce(&Self, &T) -> bool,
    ) -> Result<T, T> {
        let checkpoint = self.checkpoint();
        let mut trial_locals = locals.clone();
        let result = inference(self, &mut trial_locals);
        if accept(self, &result) {
            *locals = trial_locals;
            Ok(result)
        } else {
            self.rollback(checkpoint);
            Err(result)
        }
    }

    fn flattened_hint_members(&self, hint: &Type) -> Vec<Type> {
        members_of(&self.expand_alias_deeply(hint, 0))
    }

    /// Steep's `:array`: a tuple of the hint is tried first, then an array of it, and an array of the elements is
    /// the fallback
    pub(crate) fn infer_array(
        &mut self,
        node: &Node<'pr>,
        hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let Some(array) = node.as_array_node() else {
            return Type::Untyped;
        };
        let elements: Vec<Node<'pr>> = array.elements().iter().collect();
        self.infer_array_values(&elements, hint, context, locals)
    }

    pub(crate) fn infer_array_values(
        &mut self,
        elements: &[Node<'pr>],
        hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        if elements.is_empty() {
            let untyped_array = array_of(Type::Untyped);
            return match hint {
                Some(hint) if steep_relation::is_subtype(self.environment, &untyped_array, hint) => {
                    self.unwrap_hint(hint)
                }
                _ => untyped_array,
            };
        }
        let Some(hint) = hint else {
            return self.infer_array_elements(elements, None, context, locals);
        };
        let hint_members = self.flattened_hint_members(hint);
        let mut tuples: Vec<Vec<Type>> = hint_members
            .iter()
            .filter_map(|member| match member {
                Type::Tuple(element_hints) => Some(element_hints.clone()),
                _ => None,
            })
            .collect();
        if tuples.is_empty() && self.options.steep_version >= SteepVersion::Version2_1 {
            tuples = self
                .to_ary_tuples(&hint_members)
                .into_iter()
                .filter_map(|converted| match converted {
                    Type::Tuple(element_hints) => Some(element_hints),
                    _ => None,
                })
                .collect();
        }
        let has_splat = elements.iter().any(|element| matches!(element, Node::SplatNode { .. }));
        if !has_splat {
            let mut has_fallback = false;
            for element_hints in &tuples {
                let tuple = Type::Tuple(element_hints.clone());
                let trial = self.try_inference(
                    locals,
                    |checker, locals| checker.infer_tuple_elements(elements, element_hints, context, locals),
                    |checker, inferred| steep_relation::is_subtype(checker.environment, inferred, &tuple),
                );
                match trial {
                    Ok(inferred) => return inferred,
                    Err(_) => has_fallback = true,
                }
            }
            if has_fallback {
                let first_hints = tuples[0].clone();
                return self.infer_tuple_elements(elements, &first_hints, context, locals);
            }
        }
        for member in &hint_members {
            let Type::ClassInstance { name, arguments } = member else {
                continue;
            };
            if name != "Array" || arguments.len() != 1 {
                continue;
            }
            let element_hint = arguments[0].clone();
            let trial = self.try_inference(
                locals,
                |checker, locals| checker.infer_array_elements(elements, Some(&element_hint), context, locals),
                |checker, inferred| steep_relation::is_subtype(checker.environment, inferred, hint),
            );
            if let Ok(inferred) = trial {
                return inferred;
            }
        }
        self.infer_array_elements(elements, None, context, locals)
    }

    /// Steep's `try_tuple_type`
    fn infer_tuple_elements(
        &mut self,
        elements: &[Node<'pr>],
        element_hints: &[Type],
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        Type::Tuple(
            elements
                .iter()
                .enumerate()
                .map(|(index, element)| self.infer_with_hint(element, element_hints.get(index), context, locals))
                .collect(),
        )
    }

    /// Steep's `try_array_type`
    fn infer_array_elements(
        &mut self,
        elements: &[Node<'pr>],
        element_hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let mut element_types = Vec::new();
        for element in elements {
            let inferred = self.infer_with_hint(element, element_hint, context, locals);
            match element {
                Node::SplatNode { .. } => element_types.push(element_type(&inferred)),
                _ => element_types.push(inferred),
            }
        }
        let element = Type::union(element_types);
        array_of(if is_bottom(&element) { Type::Untyped } else { element })
    }

    /// Tuples that `to_ary` of the hint members returns, which Steep 2.1's `try_convert` gives an array literal whose
    /// hint has no tuple, as `[K, V]` for `Hash::_Pair[K, V]`
    fn to_ary_tuples(&self, hint_members: &[Type]) -> Vec<Type> {
        let mut tuples = Vec::new();
        for member in hint_members {
            let (Type::ClassInstance { name, arguments } | Type::Interface { name, arguments }) = member else {
                continue;
            };
            let Some(method) = self.environment.find_instance_method(name, "to_ary") else {
                continue;
            };
            let mut substitution = crate::unification::class_substitution(self.environment, name, arguments, member);
            if let Some(interface) = self.environment.interfaces.get(name.as_str()) {
                for (type_parameter, argument) in interface.type_parameters.iter().zip(arguments) {
                    substitution
                        .variables
                        .insert(type_parameter.name.clone(), argument.clone());
                }
            }
            let Some(overload) = method.overloads.iter().find(|overload| {
                let function = &overload.function;
                function.required_positionals.is_empty()
                    && function.trailing_positionals.is_empty()
                    && function.required_keywords.is_empty()
            }) else {
                continue;
            };
            let return_type = self.expand_alias_deeply(&overload.function.return_type.substitute(&substitution), 0);
            tuples.extend(
                members_of(&return_type)
                    .into_iter()
                    .filter(|converted| matches!(converted, Type::Tuple(_))),
            );
        }
        tuples
    }

    /// Steep's `type_hash`: a record hint types the hash as a record, a member of a union hint that the hash fits is
    /// picked, and a `Hash` hint gives its keys and values their hints
    pub(crate) fn infer_hash(
        &mut self,
        node: &Node<'pr>,
        hint: Option<&Type>,
        context: &Context,
        locals: &mut Locals,
    ) -> Type {
        let hint = hint.map(|hint| self.expand_alias_deeply(hint, 0));
        match &hint {
            Some(Type::Record(fields)) => {
                let fields = fields.clone();
                if let Ok(record) = self.try_inference(
                    locals,
                    |checker, locals| checker.infer_hash_record(node, Some(&fields), context, locals),
                    |_, record| record.is_some(),
                ) {
                    return record.unwrap_or(Type::Untyped);
                }
            }
            Some(Type::Union(members)) => {
                for member in members.clone() {
                    let trial = self.try_inference(
                        locals,
                        |checker, locals| checker.infer_hash(node, Some(&member), context, locals),
                        |checker, inferred| steep_relation::is_subtype(checker.environment, inferred, &member),
                    );
                    if let Ok(inferred) = trial {
                        return inferred;
                    }
                }
            }
            _ => {}
        }
        let (key_hint, value_hint) = match &hint {
            Some(Type::ClassInstance { name, arguments }) if name == "Hash" && arguments.len() == 2 => {
                (Some(arguments[0].clone()), Some(arguments[1].clone()))
            }
            _ => (None, None),
        };
        let hash_hint = hash_of(
            key_hint.clone().unwrap_or(Type::Untyped),
            value_hint.clone().unwrap_or(Type::Untyped),
        );
        let elements = match node {
            Node::HashNode { .. } => node.as_hash_node().map(|hash| hash.elements()),
            _ => node.as_keyword_hash_node().map(|hash| hash.elements()),
        };
        let Some(elements) = elements else {
            return Type::Untyped;
        };
        let mut key_types = Vec::new();
        let mut value_types = Vec::new();
        if elements.iter().next().is_none() {
            key_types.extend(key_hint.clone());
            value_types.extend(value_hint.clone());
        }
        for element in elements.iter() {
            match element {
                Node::AssocNode { .. } => {
                    let Some(association) = element.as_assoc_node() else {
                        continue;
                    };
                    key_types.push(self.infer_with_hint(&association.key(), key_hint.as_ref(), context, locals));
                    let value = association.value();
                    value_types.push(if matches!(value, Node::ImplicitNode { .. }) {
                        self.infer_implicit_value(&value, context, locals)
                    } else {
                        self.infer_with_hint(&value, value_hint.as_ref(), context, locals)
                    });
                }
                Node::AssocSplatNode { .. } => {
                    let Some(splat) = element.as_assoc_splat_node() else {
                        continue;
                    };
                    let splatted = self.infer_optional_node(splat.value(), Some(&hash_hint), context, locals);
                    if let Type::ClassInstance { name, arguments } = &splatted
                        && name == "Hash"
                        && arguments.len() == 2
                    {
                        key_types.push(arguments[0].clone());
                        value_types.push(arguments[1].clone());
                    }
                }
                _ => {
                    self.infer(&element, context, locals);
                }
            }
        }
        let known = |types: Vec<Type>| {
            let types: Vec<Type> = types.into_iter().filter(|member| !member.is_untyped()).collect();
            if types.is_empty() { Type::Untyped } else { Type::union(types) }
        };
        hash_of(known(key_types), known(value_types))
    }

    /// Steep's `type_hash_record`: `None` when a key is not a literal. A key the record hint does not have is reported
    /// and kept as an optional key, so that the hash does not fit the record
    fn infer_hash_record(
        &mut self,
        node: &Node<'pr>,
        hint_fields: Option<&[RecordField]>,
        context: &Context,
        locals: &mut Locals,
    ) -> Option<Type> {
        let elements = match node {
            Node::HashNode { .. } => node.as_hash_node()?.elements(),
            _ => node.as_keyword_hash_node()?.elements(),
        };
        let mut record_fields = Vec::new();
        for element in elements.iter() {
            let association = element.as_assoc_node()?;
            let key = association.key();
            let symbol = key.as_symbol_node()?;
            let key_name = bytes_text(symbol.unescaped());
            let field = hint_fields.and_then(|fields| fields.iter().find(|field| field.name == key_name));
            let value = association.value();
            let value_type = if matches!(value, Node::ImplicitNode { .. }) {
                self.infer_implicit_value(&value, context, locals)
            } else {
                self.infer_with_hint(&value, field.map(|field| &field.field_type), context, locals)
            };
            let value_type = match field {
                Some(field) if value_type.is_untyped() => field.field_type.clone(),
                Some(_) => value_type,
                None => {
                    let key_span = symbol
                        .value_loc()
                        .map(|location| Span::of_location(&location))
                        .unwrap_or_else(|| Span::of_node(&key));
                    self.record_diagnostic(
                        key_span,
                        Severity::Error,
                        "Ruby::UnknownRecordKey",
                        format!(
                            "Unknown key `{}` is given to a record type",
                            Type::Literal(Literal::Symbol(key_name.clone())).steep_display()
                        ),
                    );
                    value_type
                }
            };
            record_fields.push(RecordField {
                name: key_name,
                field_type: value_type,
                required: field.is_some_and(|field| field.required),
            });
        }
        Some(Type::Record(record_fields))
    }
}
