use serde_json::{Value, json};

use super::support::common::markdown_from_index_entries;
use super::support::receiver_methods;
use super::support::static_docs::{GUESSED_TYPES_URL, keyword_documentation, keyword_documentation_uri};
use super::{RequestContext, RequestResult};
use crate::indexer::{EntryId, EntryKind};
use crate::listeners::completion::completion_item_kind;
use crate::type_checking;

pub const MAX_DOCUMENTATION_ENTRIES: usize = 10;

pub fn handle(context: &mut RequestContext<'_>, params: Value) -> RequestResult {
    let mut item = params;
    let data = item.get("data").cloned().unwrap_or(Value::Null);

    if is_truthy(&data["delegateCompletion"]) || is_truthy(&data["skip_resolve"]) {
        return Ok(item);
    }

    let mut label = item.get("label").and_then(Value::as_str).unwrap_or_default().to_string();
    if is_truthy(&data["keyword"]) {
        keyword_resolve(&mut item, &label);
        return Ok(item);
    }

    let global_state = &*context.global_state;
    let signature_markdown = truthy_string(&data["receiver_type"]).and_then(|receiver_type| {
        let environment = global_state.type_checking.environment.as_deref()?;
        receiver_methods::method_markdown(environment, receiver_type, &label)
    });

    let mut index = global_state.index();
    let mut entries: Vec<EntryId> = index.get_owned(&label).unwrap_or_default();

    if let Some(owner_name) = truthy_string(&data["owner_name"]) {
        entries.retain(|id| {
            let entry = index.entry(*id);
            let kind_matches = entry.is_member()
                || matches!(
                    entry.kind,
                    EntryKind::InstanceVariable { .. } | EntryKind::MethodAlias { .. } | EntryKind::ClassVariable { .. }
                );
            kind_matches && entry.owner() == Some(owner_name)
        });
    }

    if let Some(first_entry) = entries.first().map(|id| index.entry(*id))
        && first_entry.is_member()
    {
        label.push_str(&first_entry.decorated_parameters());
        label.push_str(&first_entry.formatted_signatures());
    }

    let extra_links = truthy_string(&data["guessed_type"]).map(|guessed_type| {
        label.push_str(&format!("\n\nGuessed receiver: {guessed_type}"));
        format!("[Learn more about guessed types]({GUESSED_TYPES_URL})")
    });

    if let Some(signature_markdown) = signature_markdown {
        let index_markdown = (!entries.is_empty()).then(|| {
            markdown_from_index_entries(&mut index, &label, &entries, Some(MAX_DOCUMENTATION_ENTRIES), extra_links.as_deref())
        });
        let value = type_checking::merge_hover(Some(signature_markdown), index_markdown);
        item["documentation"] = json!({ "kind": "markdown", "value": value });
        return Ok(item);
    }

    if item.get("kind").and_then(Value::as_u64) != Some(u64::from(completion_item_kind::FILE)) {
        let value =
            markdown_from_index_entries(&mut index, &label, &entries, Some(MAX_DOCUMENTATION_ENTRIES), extra_links.as_deref());
        item["documentation"] = json!({ "kind": "markdown", "value": value });
    }

    Ok(item)
}

fn keyword_resolve(item: &mut Value, keyword: &str) {
    let Some(content) = keyword_documentation(keyword) else { return };
    let documentation_uri = keyword_documentation_uri(keyword);
    item["documentation"] = json!({
        "kind": "markdown",
        "value": format!("```ruby\n{keyword}\n```\n\n[Read more]({documentation_uri})\n\n{content}"),
    });
}

fn is_truthy(value: &Value) -> bool {
    !matches!(value, Value::Null | Value::Bool(false))
}

fn truthy_string(value: &Value) -> Option<&str> {
    if is_truthy(value) { value.as_str() } else { None }
}
