use std::path::Path;

use rbs_environment::{Environment, Loader, Location, MethodKind, NameKind, SignatureTarget};

const SIGNATURE_PATH: &str = "sig/app.rbs";

const SIGNATURE: &str = r#"use Billing::Invoice as Bill

module Billing
  class Invoice
  end

  interface _Payable
  end

  type amount = Integer
end

class Integer
end

class Array[unchecked out Elem]
end

module Billing
  class Customer
    include _Payable

    DEFAULT_LIMIT: Integer

    attr_reader name: String
    attr_writer limit: Integer

    def invoices: () -> Array[Invoice]
    def self.find: (amount) -> Customer
    def self?.helper: () -> Bill
    alias bills invoices
  end
end
"#;

fn environment() -> Environment {
    let mut loader = Loader::new();
    loader.add_signature_source(Path::new(SIGNATURE_PATH), SIGNATURE);
    loader.finish()
}

fn targets_at(line: u32, text_on_line: &str) -> Vec<SignatureTarget> {
    let line_text = SIGNATURE.lines().nth(line as usize).unwrap();
    let column = line_text.find(text_on_line).unwrap() as u32;
    environment().signature_targets_at(
        Path::new(SIGNATURE_PATH),
        SIGNATURE,
        |location: &Location| {
            (location.start.line, location.start.byte_column) <= (line, column)
                && (line, column) < (location.end.line, location.end.byte_column)
        },
    )
}

fn type_reference(kind: NameKind, name: &str) -> SignatureTarget {
    SignatureTarget::TypeReference {
        kind,
        name: name.to_string(),
    }
}

fn method(kind: MethodKind, name: &str) -> SignatureTarget {
    SignatureTarget::Method {
        owner: "Billing::Customer".to_string(),
        kind,
        name: name.to_string(),
    }
}

#[test]
fn type_argument_is_resolved_in_the_enclosing_namespace() {
    assert_eq!(
        targets_at(27, "Invoice]"),
        vec![type_reference(NameKind::Class, "Billing::Invoice")]
    );
}

#[test]
fn generic_class_is_the_type_outside_of_its_arguments() {
    assert_eq!(
        targets_at(27, "Array"),
        vec![type_reference(NameKind::Class, "Array")]
    );
}

#[test]
fn name_given_by_use_is_resolved() {
    assert_eq!(
        targets_at(29, "Bill"),
        vec![type_reference(NameKind::Class, "Billing::Invoice")]
    );
}

#[test]
fn type_alias_and_interface_are_referenced_by_kind() {
    assert_eq!(
        targets_at(28, "amount"),
        vec![type_reference(NameKind::Alias, "Billing::amount")]
    );
    assert_eq!(
        targets_at(20, "_Payable"),
        vec![type_reference(NameKind::Interface, "Billing::_Payable")]
    );
}

#[test]
fn unknown_type_has_no_target() {
    let source = "class Customer\n  def invoices: () -> Unknown\nend\n";
    let targets = environment().signature_targets_at(
        Path::new(SIGNATURE_PATH),
        source,
        |location: &Location| {
            location.start.line == 1
                && location.start.byte_column <= 22
                && 22 < location.end.byte_column
        },
    );
    assert_eq!(targets, Vec::new());
}

#[test]
fn method_name_is_the_method_of_its_owner() {
    assert_eq!(
        targets_at(27, "invoices"),
        vec![method(MethodKind::Instance, "invoices")]
    );
    assert_eq!(
        targets_at(28, "find"),
        vec![method(MethodKind::Singleton, "find")]
    );
}

#[test]
fn module_function_is_both_instance_and_singleton_method() {
    assert_eq!(
        targets_at(29, "helper"),
        vec![
            method(MethodKind::Instance, "helper"),
            method(MethodKind::Singleton, "helper")
        ]
    );
}

#[test]
fn alias_names_are_methods() {
    assert_eq!(
        targets_at(30, "bills"),
        vec![method(MethodKind::Instance, "bills")]
    );
    assert_eq!(
        targets_at(30, "invoices"),
        vec![method(MethodKind::Instance, "invoices")]
    );
}

#[test]
fn attribute_writer_is_the_setter_method() {
    assert_eq!(
        targets_at(24, "name"),
        vec![method(MethodKind::Instance, "name")]
    );
    assert_eq!(
        targets_at(25, "limit"),
        vec![method(MethodKind::Instance, "limit=")]
    );
}

#[test]
fn declaration_names_are_declarations() {
    assert_eq!(
        targets_at(19, "Customer"),
        vec![SignatureTarget::ClassDeclaration(
            "Billing::Customer".to_string()
        )]
    );
    assert_eq!(
        targets_at(22, "DEFAULT_LIMIT"),
        vec![SignatureTarget::ConstantDeclaration(
            "Billing::Customer::DEFAULT_LIMIT".to_string()
        )]
    );
}

#[test]
fn keyword_has_no_target() {
    assert_eq!(targets_at(27, "def"), Vec::new());
}
