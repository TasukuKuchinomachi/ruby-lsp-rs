//! Follows Steep's `Subtyping::Check#check_type0` (steep 2.0.0 `lib/steep/subtyping/check.rb`) only to tell the
//! relations it visits on the way to a failure, which Steep prints under a type mismatch through
//! `Diagnostic::Ruby::ResultPrinter#detail_lines`. Whether a mismatch is reported is still decided by `subtyping`

use std::collections::HashSet;

use rbs_environment::{Environment, Literal, Type, Variance};

use crate::type_utilities::{array_of, hash_of};

/// Steep's `Check::ABORT_LIMIT`
const ABORT_LIMIT: usize = 50;

/// One relation of the failure path, root first; `None` is a relation `ResultPrinter#relation_message` does not print
type FailurePath = Vec<Option<String>>;

/// The lines Steep prints under the header of a mismatch of `subtype <: supertype`, each indented by its depth, or
/// `None` when Steep's check would not fail
pub(crate) fn detail_lines(environment: &Environment, subtype: &Type, supertype: &Type) -> Option<String> {
    let mut checker = RelationChecker { environment, assumptions: HashSet::new() };
    let path = checker.check(subtype, supertype)?;
    let lines: Vec<String> = path
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, message)| format!("{}{message}", "  ".repeat(index + 1)))
        .collect();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

/// Whether Steep's check of `subtype <: supertype` succeeds
pub(crate) fn is_subtype(environment: &Environment, subtype: &Type, supertype: &Type) -> bool {
    RelationChecker { environment, assumptions: HashSet::new() }.check(subtype, supertype).is_none()
}

/// `header` followed by the detail lines of `subtype <: supertype`, as Steep's `Helper#full_message`
pub(crate) fn with_detail_lines(header: String, environment: &Environment, subtype: &Type, supertype: &Type) -> String {
    match detail_lines(environment, subtype, supertype) {
        Some(details) => format!("{header}\n{details}"),
        None => header,
    }
}

struct RelationChecker<'a> {
    environment: &'a Environment,
    assumptions: HashSet<(Type, Type)>,
}

fn relation_line(subtype: &Type, supertype: &Type) -> Option<String> {
    Some(format!("{} <: {}", subtype.steep_display(), supertype.steep_display()))
}

fn failure(subtype: &Type, supertype: &Type) -> Option<FailurePath> {
    Some(vec![relation_line(subtype, supertype)])
}

/// `Result::Expand`, `Result::All` and `Result::Any` put their own relation in front of the path of the failed branch
fn prepend(head: Option<String>, mut path: FailurePath) -> FailurePath {
    path.insert(0, head);
    path
}

impl RelationChecker<'_> {
    fn check(&mut self, subtype: &Type, supertype: &Type) -> Option<FailurePath> {
        if self.assumptions.len() > ABORT_LIMIT {
            return failure(subtype, supertype);
        }
        let relation = (subtype.clone(), supertype.clone());
        if self.assumptions.contains(&relation) {
            return None;
        }
        self.assumptions.insert(relation.clone());
        let result = self.check0(subtype, supertype);
        self.assumptions.remove(&relation);
        result
    }

    fn expand(&mut self, subtype: &Type, supertype: &Type, child: (Type, Type)) -> Option<FailurePath> {
        let path = self.check(&child.0, &child.1)?;
        Some(prepend(relation_line(subtype, supertype), path))
    }

    /// `Result::All`: fails with the first failing branch
    fn all(&mut self, subtype: &Type, supertype: &Type, branches: Vec<(Type, Type)>) -> Option<FailurePath> {
        for (branch_subtype, branch_supertype) in branches {
            if let Some(path) = self.check(&branch_subtype, &branch_supertype) {
                return Some(prepend(relation_line(subtype, supertype), path));
            }
        }
        None
    }

    /// `Result::Any`: succeeds with any branch, and otherwise fails with the first one
    fn any(&mut self, subtype: &Type, supertype: &Type, branches: Vec<(Type, Type)>) -> Option<FailurePath> {
        let mut first_failure = None;
        for (branch_subtype, branch_supertype) in branches {
            match self.check(&branch_subtype, &branch_supertype) {
                None => return None,
                Some(path) => {
                    first_failure.get_or_insert(path);
                }
            }
        }
        let head = relation_line(subtype, supertype);
        Some(match first_failure {
            Some(path) => prepend(head, path),
            None => vec![head],
        })
    }

    fn check0(&mut self, subtype: &Type, supertype: &Type) -> Option<FailurePath> {
        if self.same_type(subtype, supertype) {
            return None;
        }
        let boolean = || Type::union(vec![Type::Literal(Literal::True), Type::Literal(Literal::False)]);
        match (subtype, supertype) {
            (Type::Untyped, _) | (_, Type::Untyped) | (_, Type::Void | Type::Top) | (Type::Bottom, _) => None,
            (_, Type::Bool) => self.expand(subtype, supertype, (subtype.clone(), boolean())),
            (Type::Bool, _) => self.expand(subtype, supertype, (boolean(), supertype.clone())),
            (Type::Alias { name, arguments }, _) => match self.environment.expand_alias(name, arguments) {
                Some(expanded) => self.expand(subtype, supertype, (expanded, supertype.clone())),
                None => failure(subtype, supertype),
            },
            (_, Type::Alias { name, arguments }) => match self.environment.expand_alias(name, arguments) {
                Some(expanded) => self.expand(subtype, supertype, (subtype.clone(), expanded)),
                None => failure(subtype, supertype),
            },
            (Type::Intersection(subtype_members), Type::Union(supertype_members)) => {
                let head = relation_line(subtype, supertype);
                let by_supertype = self.any(
                    subtype,
                    supertype,
                    supertype_members.iter().map(|member| (subtype.clone(), member.clone())).collect(),
                )?;
                let by_subtype = self.any(
                    subtype,
                    supertype,
                    subtype_members.iter().map(|member| (member.clone(), supertype.clone())).collect(),
                );
                by_subtype.map(|_| prepend(head, by_supertype))
            }
            (_, Type::Intersection(members)) => {
                self.all(subtype, supertype, members.iter().map(|member| (subtype.clone(), member.clone())).collect())
            }
            (Type::Union(members), _) => {
                self.all(subtype, supertype, members.iter().map(|member| (member.clone(), supertype.clone())).collect())
            }
            (Type::Intersection(members), _) => {
                self.any(subtype, supertype, members.iter().map(|member| (member.clone(), supertype.clone())).collect())
            }
            (_, Type::Union(members)) => {
                self.any(subtype, supertype, members.iter().map(|member| (subtype.clone(), member.clone())).collect())
            }
            (_, Type::Interface { name, .. }) => self.interface(subtype, supertype, name),
            (
                Type::ClassInstance { .. } | Type::ClassSingleton { .. } | Type::Interface { .. },
                Type::ClassInstance { .. } | Type::ClassSingleton { .. },
            ) => self.nominal(subtype, supertype),
            (Type::Proc(subtype_proc), Type::Proc(supertype_proc)) => {
                let subtype_return = subtype_proc.function.return_type.clone();
                let supertype_return = supertype_proc.function.return_type.clone();
                let path = self.check(&subtype_return, &supertype_return)?;
                Some(prepend(relation_line(subtype, supertype), prepend(None, path)))
            }
            (Type::Tuple(subtype_elements), Type::Tuple(supertype_elements)) => {
                if subtype_elements.len() != supertype_elements.len() {
                    return failure(subtype, supertype);
                }
                let pairs = subtype_elements.iter().cloned().zip(supertype_elements.iter().cloned()).collect();
                self.all(subtype, supertype, pairs)
            }
            (Type::Tuple(elements), Type::ClassInstance { name, arguments }) if name == "Array" && arguments.len() == 1 => {
                self.expand(subtype, supertype, (Type::union(elements.clone()), arguments[0].clone()))
            }
            (Type::Tuple(elements), _) => {
                self.any(subtype, supertype, vec![(array_of(Type::union(elements.clone())), supertype.clone())])
            }
            (Type::Record(subtype_fields), Type::Record(supertype_fields)) => {
                let mut branches = Vec::new();
                for field in supertype_fields {
                    let subtype_field = subtype_fields.iter().find(|subtype_field| subtype_field.name == field.name);
                    match (field.required, subtype_field) {
                        (true, _) => branches.push((
                            subtype_field.map_or(Type::Nil, |subtype_field| subtype_field.field_type.clone()),
                            field.field_type.clone(),
                        )),
                        (false, Some(subtype_field)) => branches.push((subtype_field.field_type.clone(), field.field_type.clone())),
                        (false, None) => {}
                    }
                }
                let has_unchecked_keys = subtype_fields
                    .iter()
                    .any(|subtype_field| !supertype_fields.iter().any(|field| field.name == subtype_field.name));
                match self.all(subtype, supertype, branches) {
                    Some(path) => Some(path),
                    None if has_unchecked_keys => failure(subtype, supertype),
                    None => None,
                }
            }
            (Type::Record(fields), _) => {
                let key_type = Type::union(fields.iter().map(|field| Type::Literal(Literal::Symbol(field.name.clone()))).collect());
                let value_type = Type::union(fields.iter().map(|field| field.field_type.clone()).collect());
                self.any(subtype, supertype, vec![(hash_of(key_type, value_type), supertype.clone())])
            }
            (Type::Proc(_), Type::ClassInstance { name, .. }) if name == "Proc" => None,
            (_, Type::Literal(Literal::True)) if is_instance_of(subtype, "TrueClass") => None,
            (_, Type::Literal(Literal::False)) if is_instance_of(subtype, "FalseClass") => None,
            (_, Type::Literal(_)) => failure(subtype, supertype),
            (_, Type::Nil) if is_instance_of(subtype, "NilClass") => None,
            (Type::Literal(literal), _) => {
                self.expand(subtype, supertype, (Type::instance(literal_back_type(literal)), supertype.clone()))
            }
            _ => failure(subtype, supertype),
        }
    }

    /// Steep compares `normalize_type`d types, where a union equals another with the same members in any order
    fn same_type(&self, left: &Type, right: &Type) -> bool {
        match (left, right) {
            (Type::Union(left_members), Type::Union(right_members)) => {
                left_members.iter().all(|member| right_members.iter().any(|other| self.same_type(member, other)))
                    && right_members.iter().all(|member| left_members.iter().any(|other| self.same_type(member, other)))
            }
            (Type::ClassInstance { name: left_name, arguments: left_arguments }, Type::ClassInstance { name: right_name, arguments: right_arguments }) => {
                self.environment.resolve_class_name(left_name).unwrap_or(left_name)
                    == self.environment.resolve_class_name(right_name).unwrap_or(right_name)
                    && left_arguments.len() == right_arguments.len()
                    && left_arguments.iter().zip(right_arguments).all(|(left, right)| self.same_type(left, right))
            }
            (Type::ClassSingleton { name: left_name }, Type::ClassSingleton { name: right_name }) => {
                self.environment.resolve_class_name(left_name).unwrap_or(left_name)
                    == self.environment.resolve_class_name(right_name).unwrap_or(right_name)
            }
            _ => left == right,
        }
    }

    fn nominal(&mut self, subtype: &Type, supertype: &Type) -> Option<FailurePath> {
        let resolved = |name: &str| self.environment.resolve_class_name(name).unwrap_or(name).to_string();
        match (subtype, supertype) {
            (
                Type::ClassInstance { name, arguments },
                Type::ClassInstance { name: supertype_name, arguments: supertype_arguments },
            ) if resolved(name) == resolved(supertype_name) => {
                if arguments.is_empty() || supertype_arguments.is_empty() {
                    return None;
                }
                let variances: Vec<Variance> = self
                    .environment
                    .classes
                    .get(&resolved(supertype_name))
                    .map(|declaration| declaration.type_parameters.iter().map(|parameter| parameter.variance).collect())
                    .unwrap_or_default();
                let mut branches = Vec::new();
                for (index, (argument, supertype_argument)) in arguments.iter().zip(supertype_arguments).enumerate() {
                    match variances.get(index).copied().unwrap_or_default() {
                        Variance::Covariant => branches.push((argument.clone(), supertype_argument.clone())),
                        Variance::Contravariant => branches.push((supertype_argument.clone(), argument.clone())),
                        Variance::Invariant => {
                            branches.push((argument.clone(), supertype_argument.clone()));
                            branches.push((supertype_argument.clone(), argument.clone()));
                        }
                    }
                }
                self.all(subtype, supertype, branches)
            }
            (Type::ClassSingleton { name }, Type::ClassSingleton { name: supertype_name })
                if resolved(name) == resolved(supertype_name) =>
            {
                None
            }
            _ => {
                let possible_subtypes = match subtype {
                    Type::ClassInstance { name, arguments } => self.environment.one_instance_super_types(name, arguments),
                    Type::ClassSingleton { name } => self.environment.one_singleton_super_types(name),
                    _ => Vec::new(),
                };
                if possible_subtypes.is_empty() {
                    return failure(subtype, supertype);
                }
                let branches = possible_subtypes.into_iter().map(|possible| (possible, supertype.clone())).collect();
                self.any(subtype, supertype, branches)
            }
        }
    }

    /// `Expand` around `check_interface`, whose interface relation is not printed. A method the subtype lacks fails
    /// the relation; the method types themselves are not compared here
    fn interface(&mut self, subtype: &Type, supertype: &Type, interface_name: &str) -> Option<FailurePath> {
        let class_name = match subtype {
            Type::ClassInstance { name, .. } => name.as_str(),
            Type::Literal(literal) => literal_back_type(literal),
            Type::Nil => "NilClass",
            Type::Tuple(_) => "Array",
            Type::Record(_) => "Hash",
            Type::Interface { name, .. } if name == interface_name => return None,
            _ => return Some(vec![relation_line(subtype, supertype), None]),
        };
        let mut method_names = Vec::new();
        collect_interface_method_names(self.environment, interface_name, &mut method_names, 0);
        let missing = method_names
            .iter()
            .any(|method_name| self.environment.find_instance_method(class_name, method_name).is_none());
        missing.then(|| vec![relation_line(subtype, supertype), None])
    }
}

fn is_instance_of(target: &Type, class_name: &str) -> bool {
    matches!(target, Type::ClassInstance { name, .. } if name == class_name)
}

/// Steep's `Literal#back_type`
fn literal_back_type(literal: &Literal) -> &'static str {
    match literal {
        Literal::Integer(_) => "Integer",
        Literal::String(_) => "String",
        Literal::Symbol(_) => "Symbol",
        Literal::True => "TrueClass",
        Literal::False => "FalseClass",
    }
}

fn collect_interface_method_names(environment: &Environment, interface_name: &str, method_names: &mut Vec<String>, depth: usize) {
    if depth > 8 {
        return;
    }
    let Some(declaration) = environment.interfaces.get(interface_name) else {
        return;
    };
    method_names.extend(declaration.methods.keys().cloned());
    for included in &declaration.includes {
        collect_interface_method_names(environment, &included.name, method_names, depth + 1);
    }
}
