mod support;

fn receiver_type(checked: &support::Checked, receiver: &str) -> Option<String> {
    let start = checked.source.rfind(receiver)?;
    checked
        .analysis
        .receiver_type(start, start + receiver.len())
        .map(ToString::to_string)
}

#[test]
fn records_the_receiver_types_of_incomplete_calls() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
def incomplete
  box = Box.build(1)
  box.
end
Greeter.new("a").maybe_name&.
"#,
    );
    assert_eq!(receiver_type(&checked, "box").as_deref(), Some("Box"));
    assert_eq!(
        receiver_type(&checked, "Greeter.new(\"a\").maybe_name").as_deref(),
        Some("String?")
    );
}

#[test]
fn records_the_receiver_types_of_calls_with_unfinished_arguments() {
    let environment = environment_or_skip!();
    let checked = support::check(
        environment,
        r#"
def unfinished
  greeter = Greeter.new("a")
  greeter.greet(
end
"#,
    );
    assert_eq!(
        receiver_type(&checked, "greeter").as_deref(),
        Some("Greeter")
    );
}
