require_relative "steep_services"

CompletionProvider = Steep::Services::CompletionProvider

def range(range)
  "#{range.start.line}:#{range.start.column}-#{range.end.line}:#{range.end.column}"
end

def member_location(member)
  SteepServices.member_location(member)
end

def item(item)
  case item
  when CompletionProvider::LocalVariableItem, CompletionProvider::InstanceVariableItem
    { "kind" => item.class.name.split("::").last, "identifier" => item.identifier.to_s, "range" => range(item.range), "type" => item.type.to_s }
  when CompletionProvider::KeywordArgumentItem
    { "kind" => "KeywordArgumentItem", "identifier" => item.identifier.to_s, "range" => range(item.range) }
  when CompletionProvider::ConstantItem
    kind = if item.class? then "class" elsif item.module? then "module" else "constant" end
    {
      "kind" => "ConstantItem",
      "identifier" => item.identifier.to_s,
      "range" => range(item.range),
      "type" => item.type.to_s,
      "full_name" => item.full_name.to_s,
      "constant_kind" => kind,
      "decl" => SteepServices.declaration(item.decl),
      "deprecated" => item.deprecated?
    }
  when CompletionProvider::SimpleMethodNameItem
    {
      "kind" => "SimpleMethodNameItem",
      "identifier" => item.identifier.to_s,
      "range" => range(item.range),
      "receiver_type" => item.receiver_type.to_s,
      "method_name" => item.method_name.to_s,
      "method_types" => item.method_types.map(&:to_s),
      "member" => member_location(item.method_member),
      "deprecated" => item.deprecated
    }
  when CompletionProvider::ComplexMethodNameItem
    {
      "kind" => "ComplexMethodNameItem",
      "identifier" => item.identifier.to_s,
      "range" => range(item.range),
      "receiver_type" => item.receiver_type.to_s,
      "method_types" => item.method_types.map(&:to_s),
      "method_decls" => item.method_decls.map { |decl| [decl.method_name.to_s, member_location(decl.method_def.member)] }
    }
  when CompletionProvider::GeneratedMethodNameItem
    {
      "kind" => "GeneratedMethodNameItem",
      "identifier" => item.identifier.to_s,
      "range" => range(item.range),
      "receiver_type" => item.receiver_type.to_s,
      "method_types" => item.method_types.map(&:to_s)
    }
  when CompletionProvider::TypeNameItem
    {
      "kind" => "TypeNameItem",
      "absolute_type_name" => item.absolute_type_name.to_s,
      "relative_type_name" => item.relative_type_name.to_s,
      "range" => range(item.range)
    }
  when CompletionProvider::TextItem
    { "kind" => "TextItem", "label" => item.label, "text" => item.text, "help_text" => item.help_text, "range" => range(item.range) }
  else
    raise "unexpected item: #{item.class}"
  end
end

def items(items)
  SteepServices.summarize(items.map { |value| item(value) })
end

def ruby_completion(service, target, path, line, column)
  file = service.source_files[path] or return
  subtyping = service.signature_services.fetch(target.name).current_subtyping or return
  provider = CompletionProvider::Ruby.new(source_text: file.content, path: path, subtyping: subtyping)
  if (prefix_size, found = provider.run_at_comment(line: line, column: column))
    { "prefix_size" => prefix_size, "items" => items(found) }
  else
    found =
      begin
        provider.run(line: line, column: column)
      rescue Parser::SyntaxError
        []
      end
    { "items" => items(found) } unless found.empty?
  end
end

def rbs_completion(service, target, path, line, column)
  signature = service.signature_services[target.name] or return
  prefix_size, type_names = CompletionProvider::RBS.new(path, signature).run(line, column)
  return if type_names.empty?

  {
    "prefix_size" => prefix_size,
    "type_names" => type_names.map { |absolute, relative| [absolute.to_s, relative.to_s] }.sort
  }
end

SteepServices.each_query(ARGV) do |project, service, paths, directory|
  paths.each do |path|
    text = (project.base_dir + path).read
    SteepServices.query_positions(text).each do |line, column|
      result =
        begin
          if (target = project.target_for_inline_source_path(path) || project.target_for_source_path(path))
            ruby_completion(service, target, path, line, column)
          elsif (target = project.target_for_signature_path(path))
            rbs_completion(service, target, path, line, column)
          end
        rescue => exception
          { "error" => exception.class.name }
        end
      SteepServices.print_result(directory, path, line, column, result) if result
    end
  end
end
