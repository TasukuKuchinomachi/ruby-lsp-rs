# Generates src/requests/support/prism_inspect_fields.rs from Prism's config.yml, mirroring
# templates/lib/prism/inspect_visitor.rb.erb
require "yaml"

config_path = ARGV.fetch(0)
output_path = ARGV.fetch(1)
config = YAML.load_file(config_path)

flags_by_name = config.fetch("flags").to_h { |flag| [flag.fetch("name"), flag.fetch("values").map { |value| value.fetch("name") }] }
GENERIC_KINDS = ["non-void expression", "pattern expression", "Node"].freeze

def snake_case(name)
  name.gsub(/(?<=.)([A-Z])/, '_\1').downcase
end

def typed?(field)
  kind = field["kind"]
  kind.is_a?(String) && !GENERIC_KINDS.include?(kind)
end

output = +""
output << "use ruby_prism::Node;\n\n"
output << "use super::prism_inspect::{\n"
output << "    Field, Inspector, inspect_constant, inspect_constant_list, inspect_flags, inspect_float, inspect_integer,\n"
output << "    inspect_optional_constant, inspect_string,\n"
output << "};\n\n"
output << "pub fn node_fields<'pr>(inspector: &Inspector<'_>, node: &Node<'pr>) -> (&'static str, Vec<Field<'pr>>) {\n"
output << "    match node {\n"

config.fetch("nodes").each do |node|
  name = node.fetch("name")
  fields = node.fetch("fields", [])
  flag_values = node["flags"] ? flags_by_name.fetch(node["flags"]) : []
  binding_name = "typed_node"

  output << "        Node::#{name} { .. } => {\n"
  output << "            let #{binding_name} = node.as_#{snake_case(name)}().expect(\"variant was matched\");\n"
  output << "            (\n"
  output << "                \"#{name}\",\n"
  output << "                vec![\n"
  flag_entries = ["(#{binding_name}.flags() & 1 != 0, \"newline\")", "(#{binding_name}.flags() & 2 != 0, \"static_literal\")"]
  flag_values.each do |value|
    flag_entries << "(#{binding_name}.is_#{value.downcase}(), \"#{value.downcase}\")"
  end
  output << "                    Field::Value(\"flags\", inspect_flags(&[#{flag_entries.join(", ")}])),\n"

  fields.each do |field|
    field_name = field.fetch("name")
    accessor = "#{binding_name}.#{field_name}()"
    expression =
      case field.fetch("type")
      when "node"
        node_expression = typed?(field) ? "#{accessor}.as_node()" : accessor
        "Field::Child(\"#{field_name}\", Some(#{node_expression}))"
      when "node?"
        node_expression = typed?(field) ? "#{accessor}.map(|child| child.as_node())" : accessor
        "Field::Child(\"#{field_name}\", #{node_expression})"
      when "node[]"
        "Field::Children(\"#{field_name}\", #{accessor}.iter().collect())"
      when "string"
        binary = flag_values.include?("FORCED_BINARY_ENCODING") ? "#{binding_name}.is_forced_binary_encoding()" : "false"
        "Field::Value(\"#{field_name}\", inspect_string(#{accessor}, #{binary}))"
      when "constant"
        "Field::Value(\"#{field_name}\", inspect_constant(&#{accessor}))"
      when "constant?"
        "Field::Value(\"#{field_name}\", inspect_optional_constant(#{accessor}))"
      when "constant[]"
        "Field::Value(\"#{field_name}\", inspect_constant_list(&#{accessor}))"
      when "location"
        "Field::Value(\"#{field_name}\", inspector.location(#{accessor}))"
      when "location?"
        "Field::Value(\"#{field_name}\", inspector.optional_location(#{accessor}))"
      when "uint8", "uint32"
        "Field::Value(\"#{field_name}\", #{accessor}.to_string())"
      when "integer"
        "Field::Value(\"#{field_name}\", inspect_integer(&#{accessor}))"
      when "double"
        "Field::Value(\"#{field_name}\", inspect_float(#{accessor}))"
      else
        raise "Unknown field type #{field.fetch("type")} for #{name}##{field_name}"
      end
    output << "                    #{expression},\n"
  end
  output << "                ],\n"
  output << "            )\n"
  output << "        }\n"
end

output << "    }\n"
output << "}\n"

File.write(output_path, output)
