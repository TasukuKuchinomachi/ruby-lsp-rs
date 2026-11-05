use std::rc::Rc;

use rbs_definition::ast::{Callable, Type as RbsType, TypeName};
use steep_interface::types::Type;

use super::{Pair, TypeConstruction};
use crate::{inference::InferenceRule, ruby::node::Node};

impl<'checker> TypeConstruction<'checker> {
    fn infers(&self, rule: InferenceRule) -> bool {
        self.inference.rules.contains(rule)
    }

    fn record_inference_rule(&self, node: &Node, rule: InferenceRule) {
        self.typing.add_inference_rule_application(node.id, rule);
    }

    pub(super) fn synthesize_receiver(
        &self,
        receiver: &Rc<Node>,
        method_name: &str,
        hint: Option<&Type>,
    ) -> Pair<'checker> {
        if self.infers(InferenceRule::CollectionLiteralWithHintOfSelfReturningCall)
            && let Some(pair) =
                self.synthesize_collection_literal_with_hint_of_call(receiver, method_name, hint)
        {
            return pair;
        }
        self.synthesize(receiver, None, false)
    }

    fn synthesize_collection_literal_with_hint_of_call(
        &self,
        receiver: &Rc<Node>,
        method_name: &str,
        hint: Option<&Type>,
    ) -> Option<Pair<'checker>> {
        let hint = hint.filter(|hint| !matches!(hint, Type::Any))?;
        let class_name = match receiver.type_ {
            "array" => "::Array",
            "hash" => "::Hash",
            _ => return None,
        };
        if !self.returns_self(class_name, method_name) {
            return None;
        }
        let trial = self.with_new_typing(self.typing.new_child());
        let pair = trial.synthesize(receiver, Some(hint), false);
        if pair.constr.check_relation(&pair.type_, hint).is_failure() {
            return None;
        }
        let constr = pair.constr.save_typing();
        constr.record_inference_rule(
            receiver,
            InferenceRule::CollectionLiteralWithHintOfSelfReturningCall,
        );
        Some(Pair::new(pair.type_, constr))
    }

    fn returns_self(&self, class_name: &str, method_name: &str) -> bool {
        let Ok(definition) = self
            .builder()
            .definition_builder
            .build_instance(&TypeName::parse(class_name))
        else {
            return false;
        };
        let Some(method) = definition.methods.get(method_name) else {
            return false;
        };
        method.method_types().all(|method_type| {
            let return_type = match &method_type.type_ {
                Callable::Function(function) => &function.return_type,
                Callable::Untyped { return_type } => return_type,
            };
            matches!(return_type, RbsType::SelfType)
        })
    }
}
