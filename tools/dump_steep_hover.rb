require_relative "steep_services"

def method_call(call)
  result = {
    "kind" => call.class.name.split("::").last,
    "method_name" => call.method_name.to_s,
    "receiver_type" => call.receiver_type.to_s,
    "return_type" => call.return_type.to_s,
    "method_decls" => call.method_decls.sort_by { |decl| decl.method_name.to_s }.map do |decl|
      {
        "method_name" => decl.method_name.to_s,
        "method_type" => decl.method_type.to_s,
        "comment" => SteepServices.comment(decl.method_def.comment),
        "location" => SteepServices.member_location(decl.method_def.member)
      }
    end
  }
  if call.is_a?(Steep::TypeInference::MethodCall::Typed)
    result["actual_method_type"] = call.actual_method_type.to_s
  end
  result
end

def hover(content)
  case content
  when Steep::Services::HoverProvider::VariableContent
    { "VariableContent" => { "location" => SteepServices.location(content.location), "name" => content.name.to_s, "type" => content.type.to_s } }
  when Steep::Services::HoverProvider::TypeContent
    { "TypeContent" => { "location" => SteepServices.location(content.location), "type" => content.type.to_s } }
  when Steep::Services::HoverProvider::TypeAssertionContent
    {
      "TypeAssertionContent" => {
        "location" => SteepServices.location(content.location),
        "original_type" => content.original_type.to_s,
        "asserted_type" => content.asserted_type.to_s
      }
    }
  when Steep::Services::HoverProvider::MethodCallContent
    { "MethodCallContent" => { "location" => SteepServices.location(content.location), "method_call" => method_call(content.method_call) } }
  when Steep::Services::HoverProvider::DefinitionContent
    {
      "DefinitionContent" => {
        "location" => SteepServices.location(content.location),
        "method_name" => content.method_name.to_s,
        "method_type" => content.method_type.to_s,
        "method_types" => content.definition.method_types.map(&:to_s),
        "comments" => content.definition.comments.map { |comment| SteepServices.comment(comment) }
      }
    }
  when Steep::Services::HoverProvider::ConstantContent
    decl =
      case
      when content.class_decl then content.class_decl.primary_decl
      when content.constant_decl then content.constant_decl.decl
      when content.class_alias then content.class_alias.decl
      end
    {
      "ConstantContent" => {
        "location" => SteepServices.location(content.location),
        "full_name" => content.full_name.to_s,
        "type" => content.type.to_s,
        "decl" => SteepServices.declaration(decl),
        "comments" => content.comments.map { |comment| SteepServices.comment(comment) }
      }
    }
  when Steep::Services::HoverProvider::TypeAliasContent, Steep::Services::HoverProvider::InterfaceTypeContent, Steep::Services::HoverProvider::ClassTypeContent
    comment = content.decl.is_a?(RBS::AST::Declarations::Base) ? content.decl.comment : nil
    {
      content.class.name.split("::").last => {
        "location" => SteepServices.location(content.location),
        "decl" => SteepServices.declaration(content.decl),
        "comment" => SteepServices.comment(comment)
      }
    }
  when nil
    nil
  else
    raise "unexpected content: #{content.class}"
  end
end

SteepServices.each_query(ARGV) do |project, service, paths, directory|
  paths.each do |path|
    text = (project.base_dir + path).read
    SteepServices.query_positions(text).each do |line, column|
      result =
        begin
          hover(Steep::Services::HoverProvider.content_for(service: service, path: path, line: line, column: column))
        rescue Steep::Typing::UnknownNodeError
          nil
        rescue => exception
          { "error" => exception.class.name }
        end
      SteepServices.print_result(directory, path, line, column, result) if result
    end
  end
end
