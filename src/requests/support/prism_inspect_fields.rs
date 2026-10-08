use ruby_prism::Node;

use super::prism_inspect::{
    Field, Inspector, inspect_constant, inspect_constant_list, inspect_flags, inspect_float, inspect_integer,
    inspect_optional_constant, inspect_string,
};

pub fn node_fields<'pr>(inspector: &Inspector<'_>, node: &Node<'pr>) -> (&'static str, Vec<Field<'pr>>) {
    match node {
        Node::AliasGlobalVariableNode { .. } => {
            let typed_node = node.as_alias_global_variable_node().expect("variant was matched");
            (
                "AliasGlobalVariableNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("new_name", Some(typed_node.new_name())),
                    Field::Child("old_name", Some(typed_node.old_name())),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                ],
            )
        }
        Node::AliasMethodNode { .. } => {
            let typed_node = node.as_alias_method_node().expect("variant was matched");
            (
                "AliasMethodNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("new_name", Some(typed_node.new_name())),
                    Field::Child("old_name", Some(typed_node.old_name())),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                ],
            )
        }
        Node::AlternationPatternNode { .. } => {
            let typed_node = node.as_alternation_pattern_node().expect("variant was matched");
            (
                "AlternationPatternNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("left", Some(typed_node.left())),
                    Field::Child("right", Some(typed_node.right())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::AndNode { .. } => {
            let typed_node = node.as_and_node().expect("variant was matched");
            (
                "AndNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("left", Some(typed_node.left())),
                    Field::Child("right", Some(typed_node.right())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::ArgumentsNode { .. } => {
            let typed_node = node.as_arguments_node().expect("variant was matched");
            (
                "ArgumentsNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_contains_forwarding(), "contains_forwarding"), (typed_node.is_contains_keywords(), "contains_keywords"), (typed_node.is_contains_keyword_splat(), "contains_keyword_splat"), (typed_node.is_contains_splat(), "contains_splat"), (typed_node.is_contains_multiple_splats(), "contains_multiple_splats")])),
                    Field::Children("arguments", typed_node.arguments().iter().collect()),
                ],
            )
        }
        Node::ArrayNode { .. } => {
            let typed_node = node.as_array_node().expect("variant was matched");
            (
                "ArrayNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_contains_splat(), "contains_splat")])),
                    Field::Children("elements", typed_node.elements().iter().collect()),
                    Field::Value("opening_loc", inspector.optional_location(typed_node.opening_loc())),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                ],
            )
        }
        Node::ArrayPatternNode { .. } => {
            let typed_node = node.as_array_pattern_node().expect("variant was matched");
            (
                "ArrayPatternNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("constant", typed_node.constant()),
                    Field::Children("requireds", typed_node.requireds().iter().collect()),
                    Field::Child("rest", typed_node.rest()),
                    Field::Children("posts", typed_node.posts().iter().collect()),
                    Field::Value("opening_loc", inspector.optional_location(typed_node.opening_loc())),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                ],
            )
        }
        Node::AssocNode { .. } => {
            let typed_node = node.as_assoc_node().expect("variant was matched");
            (
                "AssocNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("key", Some(typed_node.key())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("operator_loc", inspector.optional_location(typed_node.operator_loc())),
                ],
            )
        }
        Node::AssocSplatNode { .. } => {
            let typed_node = node.as_assoc_splat_node().expect("variant was matched");
            (
                "AssocSplatNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("value", typed_node.value()),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::BackReferenceReadNode { .. } => {
            let typed_node = node.as_back_reference_read_node().expect("variant was matched");
            (
                "BackReferenceReadNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::BeginNode { .. } => {
            let typed_node = node.as_begin_node().expect("variant was matched");
            (
                "BeginNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("begin_keyword_loc", inspector.optional_location(typed_node.begin_keyword_loc())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Child("rescue_clause", typed_node.rescue_clause().map(|child| child.as_node())),
                    Field::Child("else_clause", typed_node.else_clause().map(|child| child.as_node())),
                    Field::Child("ensure_clause", typed_node.ensure_clause().map(|child| child.as_node())),
                    Field::Value("end_keyword_loc", inspector.optional_location(typed_node.end_keyword_loc())),
                ],
            )
        }
        Node::BlockArgumentNode { .. } => {
            let typed_node = node.as_block_argument_node().expect("variant was matched");
            (
                "BlockArgumentNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("expression", typed_node.expression()),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::BlockLocalVariableNode { .. } => {
            let typed_node = node.as_block_local_variable_node().expect("variant was matched");
            (
                "BlockLocalVariableNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_repeated_parameter(), "repeated_parameter")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::BlockNode { .. } => {
            let typed_node = node.as_block_node().expect("variant was matched");
            (
                "BlockNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("locals", inspect_constant_list(&typed_node.locals())),
                    Field::Child("parameters", typed_node.parameters()),
                    Field::Child("body", typed_node.body()),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                ],
            )
        }
        Node::BlockParameterNode { .. } => {
            let typed_node = node.as_block_parameter_node().expect("variant was matched");
            (
                "BlockParameterNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_repeated_parameter(), "repeated_parameter")])),
                    Field::Value("name", inspect_optional_constant(typed_node.name())),
                    Field::Value("name_loc", inspector.optional_location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::BlockParametersNode { .. } => {
            let typed_node = node.as_block_parameters_node().expect("variant was matched");
            (
                "BlockParametersNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("parameters", typed_node.parameters().map(|child| child.as_node())),
                    Field::Children("locals", typed_node.locals().iter().collect()),
                    Field::Value("opening_loc", inspector.optional_location(typed_node.opening_loc())),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                ],
            )
        }
        Node::BreakNode { .. } => {
            let typed_node = node.as_break_node().expect("variant was matched");
            (
                "BreakNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("arguments", typed_node.arguments().map(|child| child.as_node())),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                ],
            )
        }
        Node::CallAndWriteNode { .. } => {
            let typed_node = node.as_call_and_write_node().expect("variant was matched");
            (
                "CallAndWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_safe_navigation(), "safe_navigation"), (typed_node.is_variable_call(), "variable_call"), (typed_node.is_attribute_write(), "attribute_write"), (typed_node.is_ignore_visibility(), "ignore_visibility")])),
                    Field::Child("receiver", typed_node.receiver()),
                    Field::Value("call_operator_loc", inspector.optional_location(typed_node.call_operator_loc())),
                    Field::Value("message_loc", inspector.optional_location(typed_node.message_loc())),
                    Field::Value("read_name", inspect_constant(&typed_node.read_name())),
                    Field::Value("write_name", inspect_constant(&typed_node.write_name())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::CallNode { .. } => {
            let typed_node = node.as_call_node().expect("variant was matched");
            (
                "CallNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_safe_navigation(), "safe_navigation"), (typed_node.is_variable_call(), "variable_call"), (typed_node.is_attribute_write(), "attribute_write"), (typed_node.is_ignore_visibility(), "ignore_visibility")])),
                    Field::Child("receiver", typed_node.receiver()),
                    Field::Value("call_operator_loc", inspector.optional_location(typed_node.call_operator_loc())),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("message_loc", inspector.optional_location(typed_node.message_loc())),
                    Field::Value("opening_loc", inspector.optional_location(typed_node.opening_loc())),
                    Field::Child("arguments", typed_node.arguments().map(|child| child.as_node())),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                    Field::Value("equal_loc", inspector.optional_location(typed_node.equal_loc())),
                    Field::Child("block", typed_node.block()),
                ],
            )
        }
        Node::CallOperatorWriteNode { .. } => {
            let typed_node = node.as_call_operator_write_node().expect("variant was matched");
            (
                "CallOperatorWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_safe_navigation(), "safe_navigation"), (typed_node.is_variable_call(), "variable_call"), (typed_node.is_attribute_write(), "attribute_write"), (typed_node.is_ignore_visibility(), "ignore_visibility")])),
                    Field::Child("receiver", typed_node.receiver()),
                    Field::Value("call_operator_loc", inspector.optional_location(typed_node.call_operator_loc())),
                    Field::Value("message_loc", inspector.optional_location(typed_node.message_loc())),
                    Field::Value("read_name", inspect_constant(&typed_node.read_name())),
                    Field::Value("write_name", inspect_constant(&typed_node.write_name())),
                    Field::Value("binary_operator", inspect_constant(&typed_node.binary_operator())),
                    Field::Value("binary_operator_loc", inspector.location(typed_node.binary_operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::CallOrWriteNode { .. } => {
            let typed_node = node.as_call_or_write_node().expect("variant was matched");
            (
                "CallOrWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_safe_navigation(), "safe_navigation"), (typed_node.is_variable_call(), "variable_call"), (typed_node.is_attribute_write(), "attribute_write"), (typed_node.is_ignore_visibility(), "ignore_visibility")])),
                    Field::Child("receiver", typed_node.receiver()),
                    Field::Value("call_operator_loc", inspector.optional_location(typed_node.call_operator_loc())),
                    Field::Value("message_loc", inspector.optional_location(typed_node.message_loc())),
                    Field::Value("read_name", inspect_constant(&typed_node.read_name())),
                    Field::Value("write_name", inspect_constant(&typed_node.write_name())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::CallTargetNode { .. } => {
            let typed_node = node.as_call_target_node().expect("variant was matched");
            (
                "CallTargetNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_safe_navigation(), "safe_navigation"), (typed_node.is_variable_call(), "variable_call"), (typed_node.is_attribute_write(), "attribute_write"), (typed_node.is_ignore_visibility(), "ignore_visibility")])),
                    Field::Child("receiver", Some(typed_node.receiver())),
                    Field::Value("call_operator_loc", inspector.location(typed_node.call_operator_loc())),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("message_loc", inspector.location(typed_node.message_loc())),
                ],
            )
        }
        Node::CapturePatternNode { .. } => {
            let typed_node = node.as_capture_pattern_node().expect("variant was matched");
            (
                "CapturePatternNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Child("target", Some(typed_node.target().as_node())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::CaseMatchNode { .. } => {
            let typed_node = node.as_case_match_node().expect("variant was matched");
            (
                "CaseMatchNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("predicate", typed_node.predicate()),
                    Field::Children("conditions", typed_node.conditions().iter().collect()),
                    Field::Child("else_clause", typed_node.else_clause().map(|child| child.as_node())),
                    Field::Value("case_keyword_loc", inspector.location(typed_node.case_keyword_loc())),
                    Field::Value("end_keyword_loc", inspector.location(typed_node.end_keyword_loc())),
                ],
            )
        }
        Node::CaseNode { .. } => {
            let typed_node = node.as_case_node().expect("variant was matched");
            (
                "CaseNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("predicate", typed_node.predicate()),
                    Field::Children("conditions", typed_node.conditions().iter().collect()),
                    Field::Child("else_clause", typed_node.else_clause().map(|child| child.as_node())),
                    Field::Value("case_keyword_loc", inspector.location(typed_node.case_keyword_loc())),
                    Field::Value("end_keyword_loc", inspector.location(typed_node.end_keyword_loc())),
                ],
            )
        }
        Node::ClassNode { .. } => {
            let typed_node = node.as_class_node().expect("variant was matched");
            (
                "ClassNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("locals", inspect_constant_list(&typed_node.locals())),
                    Field::Value("class_keyword_loc", inspector.location(typed_node.class_keyword_loc())),
                    Field::Child("constant_path", Some(typed_node.constant_path())),
                    Field::Value("inheritance_operator_loc", inspector.optional_location(typed_node.inheritance_operator_loc())),
                    Field::Child("superclass", typed_node.superclass()),
                    Field::Child("body", typed_node.body()),
                    Field::Value("end_keyword_loc", inspector.location(typed_node.end_keyword_loc())),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::ClassVariableAndWriteNode { .. } => {
            let typed_node = node.as_class_variable_and_write_node().expect("variant was matched");
            (
                "ClassVariableAndWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::ClassVariableOperatorWriteNode { .. } => {
            let typed_node = node.as_class_variable_operator_write_node().expect("variant was matched");
            (
                "ClassVariableOperatorWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("binary_operator_loc", inspector.location(typed_node.binary_operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("binary_operator", inspect_constant(&typed_node.binary_operator())),
                ],
            )
        }
        Node::ClassVariableOrWriteNode { .. } => {
            let typed_node = node.as_class_variable_or_write_node().expect("variant was matched");
            (
                "ClassVariableOrWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::ClassVariableReadNode { .. } => {
            let typed_node = node.as_class_variable_read_node().expect("variant was matched");
            (
                "ClassVariableReadNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::ClassVariableTargetNode { .. } => {
            let typed_node = node.as_class_variable_target_node().expect("variant was matched");
            (
                "ClassVariableTargetNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::ClassVariableWriteNode { .. } => {
            let typed_node = node.as_class_variable_write_node().expect("variant was matched");
            (
                "ClassVariableWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::ConstantAndWriteNode { .. } => {
            let typed_node = node.as_constant_and_write_node().expect("variant was matched");
            (
                "ConstantAndWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::ConstantOperatorWriteNode { .. } => {
            let typed_node = node.as_constant_operator_write_node().expect("variant was matched");
            (
                "ConstantOperatorWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("binary_operator_loc", inspector.location(typed_node.binary_operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("binary_operator", inspect_constant(&typed_node.binary_operator())),
                ],
            )
        }
        Node::ConstantOrWriteNode { .. } => {
            let typed_node = node.as_constant_or_write_node().expect("variant was matched");
            (
                "ConstantOrWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::ConstantPathAndWriteNode { .. } => {
            let typed_node = node.as_constant_path_and_write_node().expect("variant was matched");
            (
                "ConstantPathAndWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("target", Some(typed_node.target().as_node())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::ConstantPathNode { .. } => {
            let typed_node = node.as_constant_path_node().expect("variant was matched");
            (
                "ConstantPathNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("parent", typed_node.parent()),
                    Field::Value("name", inspect_optional_constant(typed_node.name())),
                    Field::Value("delimiter_loc", inspector.location(typed_node.delimiter_loc())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                ],
            )
        }
        Node::ConstantPathOperatorWriteNode { .. } => {
            let typed_node = node.as_constant_path_operator_write_node().expect("variant was matched");
            (
                "ConstantPathOperatorWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("target", Some(typed_node.target().as_node())),
                    Field::Value("binary_operator_loc", inspector.location(typed_node.binary_operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("binary_operator", inspect_constant(&typed_node.binary_operator())),
                ],
            )
        }
        Node::ConstantPathOrWriteNode { .. } => {
            let typed_node = node.as_constant_path_or_write_node().expect("variant was matched");
            (
                "ConstantPathOrWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("target", Some(typed_node.target().as_node())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::ConstantPathTargetNode { .. } => {
            let typed_node = node.as_constant_path_target_node().expect("variant was matched");
            (
                "ConstantPathTargetNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("parent", typed_node.parent()),
                    Field::Value("name", inspect_optional_constant(typed_node.name())),
                    Field::Value("delimiter_loc", inspector.location(typed_node.delimiter_loc())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                ],
            )
        }
        Node::ConstantPathWriteNode { .. } => {
            let typed_node = node.as_constant_path_write_node().expect("variant was matched");
            (
                "ConstantPathWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("target", Some(typed_node.target().as_node())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::ConstantReadNode { .. } => {
            let typed_node = node.as_constant_read_node().expect("variant was matched");
            (
                "ConstantReadNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::ConstantTargetNode { .. } => {
            let typed_node = node.as_constant_target_node().expect("variant was matched");
            (
                "ConstantTargetNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::ConstantWriteNode { .. } => {
            let typed_node = node.as_constant_write_node().expect("variant was matched");
            (
                "ConstantWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::DefNode { .. } => {
            let typed_node = node.as_def_node().expect("variant was matched");
            (
                "DefNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Child("receiver", typed_node.receiver()),
                    Field::Child("parameters", typed_node.parameters().map(|child| child.as_node())),
                    Field::Child("body", typed_node.body()),
                    Field::Value("locals", inspect_constant_list(&typed_node.locals())),
                    Field::Value("def_keyword_loc", inspector.location(typed_node.def_keyword_loc())),
                    Field::Value("operator_loc", inspector.optional_location(typed_node.operator_loc())),
                    Field::Value("lparen_loc", inspector.optional_location(typed_node.lparen_loc())),
                    Field::Value("rparen_loc", inspector.optional_location(typed_node.rparen_loc())),
                    Field::Value("equal_loc", inspector.optional_location(typed_node.equal_loc())),
                    Field::Value("end_keyword_loc", inspector.optional_location(typed_node.end_keyword_loc())),
                ],
            )
        }
        Node::DefinedNode { .. } => {
            let typed_node = node.as_defined_node().expect("variant was matched");
            (
                "DefinedNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("lparen_loc", inspector.optional_location(typed_node.lparen_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("rparen_loc", inspector.optional_location(typed_node.rparen_loc())),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                ],
            )
        }
        Node::ElseNode { .. } => {
            let typed_node = node.as_else_node().expect("variant was matched");
            (
                "ElseNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("else_keyword_loc", inspector.location(typed_node.else_keyword_loc())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Value("end_keyword_loc", inspector.optional_location(typed_node.end_keyword_loc())),
                ],
            )
        }
        Node::EmbeddedStatementsNode { .. } => {
            let typed_node = node.as_embedded_statements_node().expect("variant was matched");
            (
                "EmbeddedStatementsNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                ],
            )
        }
        Node::EmbeddedVariableNode { .. } => {
            let typed_node = node.as_embedded_variable_node().expect("variant was matched");
            (
                "EmbeddedVariableNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("variable", Some(typed_node.variable())),
                ],
            )
        }
        Node::EnsureNode { .. } => {
            let typed_node = node.as_ensure_node().expect("variant was matched");
            (
                "EnsureNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("ensure_keyword_loc", inspector.location(typed_node.ensure_keyword_loc())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Value("end_keyword_loc", inspector.location(typed_node.end_keyword_loc())),
                ],
            )
        }
        Node::FalseNode { .. } => {
            let typed_node = node.as_false_node().expect("variant was matched");
            (
                "FalseNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::FindPatternNode { .. } => {
            let typed_node = node.as_find_pattern_node().expect("variant was matched");
            (
                "FindPatternNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("constant", typed_node.constant()),
                    Field::Child("left", Some(typed_node.left().as_node())),
                    Field::Children("requireds", typed_node.requireds().iter().collect()),
                    Field::Child("right", Some(typed_node.right())),
                    Field::Value("opening_loc", inspector.optional_location(typed_node.opening_loc())),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                ],
            )
        }
        Node::FlipFlopNode { .. } => {
            let typed_node = node.as_flip_flop_node().expect("variant was matched");
            (
                "FlipFlopNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_exclude_end(), "exclude_end")])),
                    Field::Child("left", typed_node.left()),
                    Field::Child("right", typed_node.right()),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::FloatNode { .. } => {
            let typed_node = node.as_float_node().expect("variant was matched");
            (
                "FloatNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("value", inspect_float(typed_node.value())),
                ],
            )
        }
        Node::ForNode { .. } => {
            let typed_node = node.as_for_node().expect("variant was matched");
            (
                "ForNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("index", Some(typed_node.index())),
                    Field::Child("collection", Some(typed_node.collection())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Value("for_keyword_loc", inspector.location(typed_node.for_keyword_loc())),
                    Field::Value("in_keyword_loc", inspector.location(typed_node.in_keyword_loc())),
                    Field::Value("do_keyword_loc", inspector.optional_location(typed_node.do_keyword_loc())),
                    Field::Value("end_keyword_loc", inspector.location(typed_node.end_keyword_loc())),
                ],
            )
        }
        Node::ForwardingArgumentsNode { .. } => {
            let typed_node = node.as_forwarding_arguments_node().expect("variant was matched");
            (
                "ForwardingArgumentsNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::ForwardingParameterNode { .. } => {
            let typed_node = node.as_forwarding_parameter_node().expect("variant was matched");
            (
                "ForwardingParameterNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::ForwardingSuperNode { .. } => {
            let typed_node = node.as_forwarding_super_node().expect("variant was matched");
            (
                "ForwardingSuperNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("block", typed_node.block().map(|child| child.as_node())),
                ],
            )
        }
        Node::GlobalVariableAndWriteNode { .. } => {
            let typed_node = node.as_global_variable_and_write_node().expect("variant was matched");
            (
                "GlobalVariableAndWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::GlobalVariableOperatorWriteNode { .. } => {
            let typed_node = node.as_global_variable_operator_write_node().expect("variant was matched");
            (
                "GlobalVariableOperatorWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("binary_operator_loc", inspector.location(typed_node.binary_operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("binary_operator", inspect_constant(&typed_node.binary_operator())),
                ],
            )
        }
        Node::GlobalVariableOrWriteNode { .. } => {
            let typed_node = node.as_global_variable_or_write_node().expect("variant was matched");
            (
                "GlobalVariableOrWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::GlobalVariableReadNode { .. } => {
            let typed_node = node.as_global_variable_read_node().expect("variant was matched");
            (
                "GlobalVariableReadNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::GlobalVariableTargetNode { .. } => {
            let typed_node = node.as_global_variable_target_node().expect("variant was matched");
            (
                "GlobalVariableTargetNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::GlobalVariableWriteNode { .. } => {
            let typed_node = node.as_global_variable_write_node().expect("variant was matched");
            (
                "GlobalVariableWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::HashNode { .. } => {
            let typed_node = node.as_hash_node().expect("variant was matched");
            (
                "HashNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Children("elements", typed_node.elements().iter().collect()),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                ],
            )
        }
        Node::HashPatternNode { .. } => {
            let typed_node = node.as_hash_pattern_node().expect("variant was matched");
            (
                "HashPatternNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("constant", typed_node.constant()),
                    Field::Children("elements", typed_node.elements().iter().collect()),
                    Field::Child("rest", typed_node.rest()),
                    Field::Value("opening_loc", inspector.optional_location(typed_node.opening_loc())),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                ],
            )
        }
        Node::IfNode { .. } => {
            let typed_node = node.as_if_node().expect("variant was matched");
            (
                "IfNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("if_keyword_loc", inspector.optional_location(typed_node.if_keyword_loc())),
                    Field::Child("predicate", Some(typed_node.predicate())),
                    Field::Value("then_keyword_loc", inspector.optional_location(typed_node.then_keyword_loc())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Child("subsequent", typed_node.subsequent()),
                    Field::Value("end_keyword_loc", inspector.optional_location(typed_node.end_keyword_loc())),
                ],
            )
        }
        Node::ImaginaryNode { .. } => {
            let typed_node = node.as_imaginary_node().expect("variant was matched");
            (
                "ImaginaryNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("numeric", Some(typed_node.numeric())),
                ],
            )
        }
        Node::ImplicitNode { .. } => {
            let typed_node = node.as_implicit_node().expect("variant was matched");
            (
                "ImplicitNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::ImplicitRestNode { .. } => {
            let typed_node = node.as_implicit_rest_node().expect("variant was matched");
            (
                "ImplicitRestNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::InNode { .. } => {
            let typed_node = node.as_in_node().expect("variant was matched");
            (
                "InNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("pattern", Some(typed_node.pattern())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Value("in_loc", inspector.location(typed_node.in_loc())),
                    Field::Value("then_loc", inspector.optional_location(typed_node.then_loc())),
                ],
            )
        }
        Node::IndexAndWriteNode { .. } => {
            let typed_node = node.as_index_and_write_node().expect("variant was matched");
            (
                "IndexAndWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_safe_navigation(), "safe_navigation"), (typed_node.is_variable_call(), "variable_call"), (typed_node.is_attribute_write(), "attribute_write"), (typed_node.is_ignore_visibility(), "ignore_visibility")])),
                    Field::Child("receiver", typed_node.receiver()),
                    Field::Value("call_operator_loc", inspector.optional_location(typed_node.call_operator_loc())),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Child("arguments", typed_node.arguments().map(|child| child.as_node())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                    Field::Child("block", typed_node.block().map(|child| child.as_node())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::IndexOperatorWriteNode { .. } => {
            let typed_node = node.as_index_operator_write_node().expect("variant was matched");
            (
                "IndexOperatorWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_safe_navigation(), "safe_navigation"), (typed_node.is_variable_call(), "variable_call"), (typed_node.is_attribute_write(), "attribute_write"), (typed_node.is_ignore_visibility(), "ignore_visibility")])),
                    Field::Child("receiver", typed_node.receiver()),
                    Field::Value("call_operator_loc", inspector.optional_location(typed_node.call_operator_loc())),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Child("arguments", typed_node.arguments().map(|child| child.as_node())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                    Field::Child("block", typed_node.block().map(|child| child.as_node())),
                    Field::Value("binary_operator", inspect_constant(&typed_node.binary_operator())),
                    Field::Value("binary_operator_loc", inspector.location(typed_node.binary_operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::IndexOrWriteNode { .. } => {
            let typed_node = node.as_index_or_write_node().expect("variant was matched");
            (
                "IndexOrWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_safe_navigation(), "safe_navigation"), (typed_node.is_variable_call(), "variable_call"), (typed_node.is_attribute_write(), "attribute_write"), (typed_node.is_ignore_visibility(), "ignore_visibility")])),
                    Field::Child("receiver", typed_node.receiver()),
                    Field::Value("call_operator_loc", inspector.optional_location(typed_node.call_operator_loc())),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Child("arguments", typed_node.arguments().map(|child| child.as_node())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                    Field::Child("block", typed_node.block().map(|child| child.as_node())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::IndexTargetNode { .. } => {
            let typed_node = node.as_index_target_node().expect("variant was matched");
            (
                "IndexTargetNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_safe_navigation(), "safe_navigation"), (typed_node.is_variable_call(), "variable_call"), (typed_node.is_attribute_write(), "attribute_write"), (typed_node.is_ignore_visibility(), "ignore_visibility")])),
                    Field::Child("receiver", Some(typed_node.receiver())),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Child("arguments", typed_node.arguments().map(|child| child.as_node())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                    Field::Child("block", typed_node.block().map(|child| child.as_node())),
                ],
            )
        }
        Node::InstanceVariableAndWriteNode { .. } => {
            let typed_node = node.as_instance_variable_and_write_node().expect("variant was matched");
            (
                "InstanceVariableAndWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::InstanceVariableOperatorWriteNode { .. } => {
            let typed_node = node.as_instance_variable_operator_write_node().expect("variant was matched");
            (
                "InstanceVariableOperatorWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("binary_operator_loc", inspector.location(typed_node.binary_operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("binary_operator", inspect_constant(&typed_node.binary_operator())),
                ],
            )
        }
        Node::InstanceVariableOrWriteNode { .. } => {
            let typed_node = node.as_instance_variable_or_write_node().expect("variant was matched");
            (
                "InstanceVariableOrWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::InstanceVariableReadNode { .. } => {
            let typed_node = node.as_instance_variable_read_node().expect("variant was matched");
            (
                "InstanceVariableReadNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::InstanceVariableTargetNode { .. } => {
            let typed_node = node.as_instance_variable_target_node().expect("variant was matched");
            (
                "InstanceVariableTargetNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::InstanceVariableWriteNode { .. } => {
            let typed_node = node.as_instance_variable_write_node().expect("variant was matched");
            (
                "InstanceVariableWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::IntegerNode { .. } => {
            let typed_node = node.as_integer_node().expect("variant was matched");
            (
                "IntegerNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_binary(), "binary"), (typed_node.is_decimal(), "decimal"), (typed_node.is_octal(), "octal"), (typed_node.is_hexadecimal(), "hexadecimal")])),
                    Field::Value("value", inspect_integer(&typed_node.value())),
                ],
            )
        }
        Node::InterpolatedMatchLastLineNode { .. } => {
            let typed_node = node.as_interpolated_match_last_line_node().expect("variant was matched");
            (
                "InterpolatedMatchLastLineNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_ignore_case(), "ignore_case"), (typed_node.is_extended(), "extended"), (typed_node.is_multi_line(), "multi_line"), (typed_node.is_once(), "once"), (typed_node.is_euc_jp(), "euc_jp"), (typed_node.is_ascii_8bit(), "ascii_8bit"), (typed_node.is_windows_31j(), "windows_31j"), (typed_node.is_utf_8(), "utf_8"), (typed_node.is_forced_utf8_encoding(), "forced_utf8_encoding"), (typed_node.is_forced_binary_encoding(), "forced_binary_encoding"), (typed_node.is_forced_us_ascii_encoding(), "forced_us_ascii_encoding")])),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Children("parts", typed_node.parts().iter().collect()),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                ],
            )
        }
        Node::InterpolatedRegularExpressionNode { .. } => {
            let typed_node = node.as_interpolated_regular_expression_node().expect("variant was matched");
            (
                "InterpolatedRegularExpressionNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_ignore_case(), "ignore_case"), (typed_node.is_extended(), "extended"), (typed_node.is_multi_line(), "multi_line"), (typed_node.is_once(), "once"), (typed_node.is_euc_jp(), "euc_jp"), (typed_node.is_ascii_8bit(), "ascii_8bit"), (typed_node.is_windows_31j(), "windows_31j"), (typed_node.is_utf_8(), "utf_8"), (typed_node.is_forced_utf8_encoding(), "forced_utf8_encoding"), (typed_node.is_forced_binary_encoding(), "forced_binary_encoding"), (typed_node.is_forced_us_ascii_encoding(), "forced_us_ascii_encoding")])),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Children("parts", typed_node.parts().iter().collect()),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                ],
            )
        }
        Node::InterpolatedStringNode { .. } => {
            let typed_node = node.as_interpolated_string_node().expect("variant was matched");
            (
                "InterpolatedStringNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_frozen(), "frozen"), (typed_node.is_mutable(), "mutable")])),
                    Field::Value("opening_loc", inspector.optional_location(typed_node.opening_loc())),
                    Field::Children("parts", typed_node.parts().iter().collect()),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                ],
            )
        }
        Node::InterpolatedSymbolNode { .. } => {
            let typed_node = node.as_interpolated_symbol_node().expect("variant was matched");
            (
                "InterpolatedSymbolNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("opening_loc", inspector.optional_location(typed_node.opening_loc())),
                    Field::Children("parts", typed_node.parts().iter().collect()),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                ],
            )
        }
        Node::InterpolatedXStringNode { .. } => {
            let typed_node = node.as_interpolated_x_string_node().expect("variant was matched");
            (
                "InterpolatedXStringNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Children("parts", typed_node.parts().iter().collect()),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                ],
            )
        }
        Node::ItLocalVariableReadNode { .. } => {
            let typed_node = node.as_it_local_variable_read_node().expect("variant was matched");
            (
                "ItLocalVariableReadNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::ItParametersNode { .. } => {
            let typed_node = node.as_it_parameters_node().expect("variant was matched");
            (
                "ItParametersNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::KeywordHashNode { .. } => {
            let typed_node = node.as_keyword_hash_node().expect("variant was matched");
            (
                "KeywordHashNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_symbol_keys(), "symbol_keys")])),
                    Field::Children("elements", typed_node.elements().iter().collect()),
                ],
            )
        }
        Node::KeywordRestParameterNode { .. } => {
            let typed_node = node.as_keyword_rest_parameter_node().expect("variant was matched");
            (
                "KeywordRestParameterNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_repeated_parameter(), "repeated_parameter")])),
                    Field::Value("name", inspect_optional_constant(typed_node.name())),
                    Field::Value("name_loc", inspector.optional_location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::LambdaNode { .. } => {
            let typed_node = node.as_lambda_node().expect("variant was matched");
            (
                "LambdaNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("locals", inspect_constant_list(&typed_node.locals())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                    Field::Child("parameters", typed_node.parameters()),
                    Field::Child("body", typed_node.body()),
                ],
            )
        }
        Node::LocalVariableAndWriteNode { .. } => {
            let typed_node = node.as_local_variable_and_write_node().expect("variant was matched");
            (
                "LocalVariableAndWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("depth", typed_node.depth().to_string()),
                ],
            )
        }
        Node::LocalVariableOperatorWriteNode { .. } => {
            let typed_node = node.as_local_variable_operator_write_node().expect("variant was matched");
            (
                "LocalVariableOperatorWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("binary_operator_loc", inspector.location(typed_node.binary_operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("binary_operator", inspect_constant(&typed_node.binary_operator())),
                    Field::Value("depth", typed_node.depth().to_string()),
                ],
            )
        }
        Node::LocalVariableOrWriteNode { .. } => {
            let typed_node = node.as_local_variable_or_write_node().expect("variant was matched");
            (
                "LocalVariableOrWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("depth", typed_node.depth().to_string()),
                ],
            )
        }
        Node::LocalVariableReadNode { .. } => {
            let typed_node = node.as_local_variable_read_node().expect("variant was matched");
            (
                "LocalVariableReadNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("depth", typed_node.depth().to_string()),
                ],
            )
        }
        Node::LocalVariableTargetNode { .. } => {
            let typed_node = node.as_local_variable_target_node().expect("variant was matched");
            (
                "LocalVariableTargetNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("depth", typed_node.depth().to_string()),
                ],
            )
        }
        Node::LocalVariableWriteNode { .. } => {
            let typed_node = node.as_local_variable_write_node().expect("variant was matched");
            (
                "LocalVariableWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("depth", typed_node.depth().to_string()),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::MatchLastLineNode { .. } => {
            let typed_node = node.as_match_last_line_node().expect("variant was matched");
            (
                "MatchLastLineNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_ignore_case(), "ignore_case"), (typed_node.is_extended(), "extended"), (typed_node.is_multi_line(), "multi_line"), (typed_node.is_once(), "once"), (typed_node.is_euc_jp(), "euc_jp"), (typed_node.is_ascii_8bit(), "ascii_8bit"), (typed_node.is_windows_31j(), "windows_31j"), (typed_node.is_utf_8(), "utf_8"), (typed_node.is_forced_utf8_encoding(), "forced_utf8_encoding"), (typed_node.is_forced_binary_encoding(), "forced_binary_encoding"), (typed_node.is_forced_us_ascii_encoding(), "forced_us_ascii_encoding")])),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Value("content_loc", inspector.location(typed_node.content_loc())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                    Field::Value("unescaped", inspect_string(typed_node.unescaped(), typed_node.is_forced_binary_encoding())),
                ],
            )
        }
        Node::MatchPredicateNode { .. } => {
            let typed_node = node.as_match_predicate_node().expect("variant was matched");
            (
                "MatchPredicateNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Child("pattern", Some(typed_node.pattern())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::MatchRequiredNode { .. } => {
            let typed_node = node.as_match_required_node().expect("variant was matched");
            (
                "MatchRequiredNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("value", Some(typed_node.value())),
                    Field::Child("pattern", Some(typed_node.pattern())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::MatchWriteNode { .. } => {
            let typed_node = node.as_match_write_node().expect("variant was matched");
            (
                "MatchWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("call", Some(typed_node.call().as_node())),
                    Field::Children("targets", typed_node.targets().iter().collect()),
                ],
            )
        }
        Node::MissingNode { .. } => {
            let typed_node = node.as_missing_node().expect("variant was matched");
            (
                "MissingNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::ModuleNode { .. } => {
            let typed_node = node.as_module_node().expect("variant was matched");
            (
                "ModuleNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("locals", inspect_constant_list(&typed_node.locals())),
                    Field::Value("module_keyword_loc", inspector.location(typed_node.module_keyword_loc())),
                    Field::Child("constant_path", Some(typed_node.constant_path())),
                    Field::Child("body", typed_node.body()),
                    Field::Value("end_keyword_loc", inspector.location(typed_node.end_keyword_loc())),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::MultiTargetNode { .. } => {
            let typed_node = node.as_multi_target_node().expect("variant was matched");
            (
                "MultiTargetNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Children("lefts", typed_node.lefts().iter().collect()),
                    Field::Child("rest", typed_node.rest()),
                    Field::Children("rights", typed_node.rights().iter().collect()),
                    Field::Value("lparen_loc", inspector.optional_location(typed_node.lparen_loc())),
                    Field::Value("rparen_loc", inspector.optional_location(typed_node.rparen_loc())),
                ],
            )
        }
        Node::MultiWriteNode { .. } => {
            let typed_node = node.as_multi_write_node().expect("variant was matched");
            (
                "MultiWriteNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Children("lefts", typed_node.lefts().iter().collect()),
                    Field::Child("rest", typed_node.rest()),
                    Field::Children("rights", typed_node.rights().iter().collect()),
                    Field::Value("lparen_loc", inspector.optional_location(typed_node.lparen_loc())),
                    Field::Value("rparen_loc", inspector.optional_location(typed_node.rparen_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::NextNode { .. } => {
            let typed_node = node.as_next_node().expect("variant was matched");
            (
                "NextNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("arguments", typed_node.arguments().map(|child| child.as_node())),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                ],
            )
        }
        Node::NilNode { .. } => {
            let typed_node = node.as_nil_node().expect("variant was matched");
            (
                "NilNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::NoKeywordsParameterNode { .. } => {
            let typed_node = node.as_no_keywords_parameter_node().expect("variant was matched");
            (
                "NoKeywordsParameterNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                ],
            )
        }
        Node::NumberedParametersNode { .. } => {
            let typed_node = node.as_numbered_parameters_node().expect("variant was matched");
            (
                "NumberedParametersNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("maximum", typed_node.maximum().to_string()),
                ],
            )
        }
        Node::NumberedReferenceReadNode { .. } => {
            let typed_node = node.as_numbered_reference_read_node().expect("variant was matched");
            (
                "NumberedReferenceReadNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("number", typed_node.number().to_string()),
                ],
            )
        }
        Node::OptionalKeywordParameterNode { .. } => {
            let typed_node = node.as_optional_keyword_parameter_node().expect("variant was matched");
            (
                "OptionalKeywordParameterNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_repeated_parameter(), "repeated_parameter")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::OptionalParameterNode { .. } => {
            let typed_node = node.as_optional_parameter_node().expect("variant was matched");
            (
                "OptionalParameterNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_repeated_parameter(), "repeated_parameter")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("value", Some(typed_node.value())),
                ],
            )
        }
        Node::OrNode { .. } => {
            let typed_node = node.as_or_node().expect("variant was matched");
            (
                "OrNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("left", Some(typed_node.left())),
                    Field::Child("right", Some(typed_node.right())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::ParametersNode { .. } => {
            let typed_node = node.as_parameters_node().expect("variant was matched");
            (
                "ParametersNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Children("requireds", typed_node.requireds().iter().collect()),
                    Field::Children("optionals", typed_node.optionals().iter().collect()),
                    Field::Child("rest", typed_node.rest()),
                    Field::Children("posts", typed_node.posts().iter().collect()),
                    Field::Children("keywords", typed_node.keywords().iter().collect()),
                    Field::Child("keyword_rest", typed_node.keyword_rest()),
                    Field::Child("block", typed_node.block().map(|child| child.as_node())),
                ],
            )
        }
        Node::ParenthesesNode { .. } => {
            let typed_node = node.as_parentheses_node().expect("variant was matched");
            (
                "ParenthesesNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_multiple_statements(), "multiple_statements")])),
                    Field::Child("body", typed_node.body()),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                ],
            )
        }
        Node::PinnedExpressionNode { .. } => {
            let typed_node = node.as_pinned_expression_node().expect("variant was matched");
            (
                "PinnedExpressionNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("expression", Some(typed_node.expression())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Value("lparen_loc", inspector.location(typed_node.lparen_loc())),
                    Field::Value("rparen_loc", inspector.location(typed_node.rparen_loc())),
                ],
            )
        }
        Node::PinnedVariableNode { .. } => {
            let typed_node = node.as_pinned_variable_node().expect("variant was matched");
            (
                "PinnedVariableNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("variable", Some(typed_node.variable())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::PostExecutionNode { .. } => {
            let typed_node = node.as_post_execution_node().expect("variant was matched");
            (
                "PostExecutionNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                ],
            )
        }
        Node::PreExecutionNode { .. } => {
            let typed_node = node.as_pre_execution_node().expect("variant was matched");
            (
                "PreExecutionNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                ],
            )
        }
        Node::ProgramNode { .. } => {
            let typed_node = node.as_program_node().expect("variant was matched");
            (
                "ProgramNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("locals", inspect_constant_list(&typed_node.locals())),
                    Field::Child("statements", Some(typed_node.statements().as_node())),
                ],
            )
        }
        Node::RangeNode { .. } => {
            let typed_node = node.as_range_node().expect("variant was matched");
            (
                "RangeNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_exclude_end(), "exclude_end")])),
                    Field::Child("left", typed_node.left()),
                    Field::Child("right", typed_node.right()),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::RationalNode { .. } => {
            let typed_node = node.as_rational_node().expect("variant was matched");
            (
                "RationalNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_binary(), "binary"), (typed_node.is_decimal(), "decimal"), (typed_node.is_octal(), "octal"), (typed_node.is_hexadecimal(), "hexadecimal")])),
                    Field::Value("numerator", inspect_integer(&typed_node.numerator())),
                    Field::Value("denominator", inspect_integer(&typed_node.denominator())),
                ],
            )
        }
        Node::RedoNode { .. } => {
            let typed_node = node.as_redo_node().expect("variant was matched");
            (
                "RedoNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::RegularExpressionNode { .. } => {
            let typed_node = node.as_regular_expression_node().expect("variant was matched");
            (
                "RegularExpressionNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_ignore_case(), "ignore_case"), (typed_node.is_extended(), "extended"), (typed_node.is_multi_line(), "multi_line"), (typed_node.is_once(), "once"), (typed_node.is_euc_jp(), "euc_jp"), (typed_node.is_ascii_8bit(), "ascii_8bit"), (typed_node.is_windows_31j(), "windows_31j"), (typed_node.is_utf_8(), "utf_8"), (typed_node.is_forced_utf8_encoding(), "forced_utf8_encoding"), (typed_node.is_forced_binary_encoding(), "forced_binary_encoding"), (typed_node.is_forced_us_ascii_encoding(), "forced_us_ascii_encoding")])),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Value("content_loc", inspector.location(typed_node.content_loc())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                    Field::Value("unescaped", inspect_string(typed_node.unescaped(), typed_node.is_forced_binary_encoding())),
                ],
            )
        }
        Node::RequiredKeywordParameterNode { .. } => {
            let typed_node = node.as_required_keyword_parameter_node().expect("variant was matched");
            (
                "RequiredKeywordParameterNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_repeated_parameter(), "repeated_parameter")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                    Field::Value("name_loc", inspector.location(typed_node.name_loc())),
                ],
            )
        }
        Node::RequiredParameterNode { .. } => {
            let typed_node = node.as_required_parameter_node().expect("variant was matched");
            (
                "RequiredParameterNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_repeated_parameter(), "repeated_parameter")])),
                    Field::Value("name", inspect_constant(&typed_node.name())),
                ],
            )
        }
        Node::RescueModifierNode { .. } => {
            let typed_node = node.as_rescue_modifier_node().expect("variant was matched");
            (
                "RescueModifierNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Child("expression", Some(typed_node.expression())),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Child("rescue_expression", Some(typed_node.rescue_expression())),
                ],
            )
        }
        Node::RescueNode { .. } => {
            let typed_node = node.as_rescue_node().expect("variant was matched");
            (
                "RescueNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Children("exceptions", typed_node.exceptions().iter().collect()),
                    Field::Value("operator_loc", inspector.optional_location(typed_node.operator_loc())),
                    Field::Child("reference", typed_node.reference()),
                    Field::Value("then_keyword_loc", inspector.optional_location(typed_node.then_keyword_loc())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Child("subsequent", typed_node.subsequent().map(|child| child.as_node())),
                ],
            )
        }
        Node::RestParameterNode { .. } => {
            let typed_node = node.as_rest_parameter_node().expect("variant was matched");
            (
                "RestParameterNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_repeated_parameter(), "repeated_parameter")])),
                    Field::Value("name", inspect_optional_constant(typed_node.name())),
                    Field::Value("name_loc", inspector.optional_location(typed_node.name_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                ],
            )
        }
        Node::RetryNode { .. } => {
            let typed_node = node.as_retry_node().expect("variant was matched");
            (
                "RetryNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::ReturnNode { .. } => {
            let typed_node = node.as_return_node().expect("variant was matched");
            (
                "ReturnNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Child("arguments", typed_node.arguments().map(|child| child.as_node())),
                ],
            )
        }
        Node::SelfNode { .. } => {
            let typed_node = node.as_self_node().expect("variant was matched");
            (
                "SelfNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::ShareableConstantNode { .. } => {
            let typed_node = node.as_shareable_constant_node().expect("variant was matched");
            (
                "ShareableConstantNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_literal(), "literal"), (typed_node.is_experimental_everything(), "experimental_everything"), (typed_node.is_experimental_copy(), "experimental_copy")])),
                    Field::Child("write", Some(typed_node.write())),
                ],
            )
        }
        Node::SingletonClassNode { .. } => {
            let typed_node = node.as_singleton_class_node().expect("variant was matched");
            (
                "SingletonClassNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("locals", inspect_constant_list(&typed_node.locals())),
                    Field::Value("class_keyword_loc", inspector.location(typed_node.class_keyword_loc())),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("expression", Some(typed_node.expression())),
                    Field::Child("body", typed_node.body()),
                    Field::Value("end_keyword_loc", inspector.location(typed_node.end_keyword_loc())),
                ],
            )
        }
        Node::SourceEncodingNode { .. } => {
            let typed_node = node.as_source_encoding_node().expect("variant was matched");
            (
                "SourceEncodingNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::SourceFileNode { .. } => {
            let typed_node = node.as_source_file_node().expect("variant was matched");
            (
                "SourceFileNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_forced_utf8_encoding(), "forced_utf8_encoding"), (typed_node.is_forced_binary_encoding(), "forced_binary_encoding"), (typed_node.is_frozen(), "frozen"), (typed_node.is_mutable(), "mutable")])),
                    Field::Value("filepath", inspect_string(typed_node.filepath(), typed_node.is_forced_binary_encoding())),
                ],
            )
        }
        Node::SourceLineNode { .. } => {
            let typed_node = node.as_source_line_node().expect("variant was matched");
            (
                "SourceLineNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::SplatNode { .. } => {
            let typed_node = node.as_splat_node().expect("variant was matched");
            (
                "SplatNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("operator_loc", inspector.location(typed_node.operator_loc())),
                    Field::Child("expression", typed_node.expression()),
                ],
            )
        }
        Node::StatementsNode { .. } => {
            let typed_node = node.as_statements_node().expect("variant was matched");
            (
                "StatementsNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Children("body", typed_node.body().iter().collect()),
                ],
            )
        }
        Node::StringNode { .. } => {
            let typed_node = node.as_string_node().expect("variant was matched");
            (
                "StringNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_forced_utf8_encoding(), "forced_utf8_encoding"), (typed_node.is_forced_binary_encoding(), "forced_binary_encoding"), (typed_node.is_frozen(), "frozen"), (typed_node.is_mutable(), "mutable")])),
                    Field::Value("opening_loc", inspector.optional_location(typed_node.opening_loc())),
                    Field::Value("content_loc", inspector.location(typed_node.content_loc())),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                    Field::Value("unescaped", inspect_string(typed_node.unescaped(), typed_node.is_forced_binary_encoding())),
                ],
            )
        }
        Node::SuperNode { .. } => {
            let typed_node = node.as_super_node().expect("variant was matched");
            (
                "SuperNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Value("lparen_loc", inspector.optional_location(typed_node.lparen_loc())),
                    Field::Child("arguments", typed_node.arguments().map(|child| child.as_node())),
                    Field::Value("rparen_loc", inspector.optional_location(typed_node.rparen_loc())),
                    Field::Child("block", typed_node.block()),
                ],
            )
        }
        Node::SymbolNode { .. } => {
            let typed_node = node.as_symbol_node().expect("variant was matched");
            (
                "SymbolNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_forced_utf8_encoding(), "forced_utf8_encoding"), (typed_node.is_forced_binary_encoding(), "forced_binary_encoding"), (typed_node.is_forced_us_ascii_encoding(), "forced_us_ascii_encoding")])),
                    Field::Value("opening_loc", inspector.optional_location(typed_node.opening_loc())),
                    Field::Value("value_loc", inspector.optional_location(typed_node.value_loc())),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                    Field::Value("unescaped", inspect_string(typed_node.unescaped(), typed_node.is_forced_binary_encoding())),
                ],
            )
        }
        Node::TrueNode { .. } => {
            let typed_node = node.as_true_node().expect("variant was matched");
            (
                "TrueNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                ],
            )
        }
        Node::UndefNode { .. } => {
            let typed_node = node.as_undef_node().expect("variant was matched");
            (
                "UndefNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Children("names", typed_node.names().iter().collect()),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                ],
            )
        }
        Node::UnlessNode { .. } => {
            let typed_node = node.as_unless_node().expect("variant was matched");
            (
                "UnlessNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Child("predicate", Some(typed_node.predicate())),
                    Field::Value("then_keyword_loc", inspector.optional_location(typed_node.then_keyword_loc())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                    Field::Child("else_clause", typed_node.else_clause().map(|child| child.as_node())),
                    Field::Value("end_keyword_loc", inspector.optional_location(typed_node.end_keyword_loc())),
                ],
            )
        }
        Node::UntilNode { .. } => {
            let typed_node = node.as_until_node().expect("variant was matched");
            (
                "UntilNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_begin_modifier(), "begin_modifier")])),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Value("do_keyword_loc", inspector.optional_location(typed_node.do_keyword_loc())),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                    Field::Child("predicate", Some(typed_node.predicate())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                ],
            )
        }
        Node::WhenNode { .. } => {
            let typed_node = node.as_when_node().expect("variant was matched");
            (
                "WhenNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Children("conditions", typed_node.conditions().iter().collect()),
                    Field::Value("then_keyword_loc", inspector.optional_location(typed_node.then_keyword_loc())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                ],
            )
        }
        Node::WhileNode { .. } => {
            let typed_node = node.as_while_node().expect("variant was matched");
            (
                "WhileNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_begin_modifier(), "begin_modifier")])),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Value("do_keyword_loc", inspector.optional_location(typed_node.do_keyword_loc())),
                    Field::Value("closing_loc", inspector.optional_location(typed_node.closing_loc())),
                    Field::Child("predicate", Some(typed_node.predicate())),
                    Field::Child("statements", typed_node.statements().map(|child| child.as_node())),
                ],
            )
        }
        Node::XStringNode { .. } => {
            let typed_node = node.as_x_string_node().expect("variant was matched");
            (
                "XStringNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal"), (typed_node.is_forced_utf8_encoding(), "forced_utf8_encoding"), (typed_node.is_forced_binary_encoding(), "forced_binary_encoding")])),
                    Field::Value("opening_loc", inspector.location(typed_node.opening_loc())),
                    Field::Value("content_loc", inspector.location(typed_node.content_loc())),
                    Field::Value("closing_loc", inspector.location(typed_node.closing_loc())),
                    Field::Value("unescaped", inspect_string(typed_node.unescaped(), typed_node.is_forced_binary_encoding())),
                ],
            )
        }
        Node::YieldNode { .. } => {
            let typed_node = node.as_yield_node().expect("variant was matched");
            (
                "YieldNode",
                vec![
                    Field::Value("flags", inspect_flags(&[(typed_node.flags() & 1 != 0, "newline"), (typed_node.flags() & 2 != 0, "static_literal")])),
                    Field::Value("keyword_loc", inspector.location(typed_node.keyword_loc())),
                    Field::Value("lparen_loc", inspector.optional_location(typed_node.lparen_loc())),
                    Field::Child("arguments", typed_node.arguments().map(|child| child.as_node())),
                    Field::Value("rparen_loc", inspector.optional_location(typed_node.rparen_loc())),
                ],
            )
        }
    }
}
