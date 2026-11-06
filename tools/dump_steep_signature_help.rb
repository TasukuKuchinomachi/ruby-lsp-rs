require_relative "steep_services"

def signature_help(service, target, path, line, column)
  file = service.source_files[path] or return
  subtyping = service.signature_services.fetch(target.name).current_subtyping or return
  source =
    begin
      Steep::Source.parse(file.content, path: file.path, factory: subtyping.factory)
        .without_unrelated_defs(line: line, column: column)
    rescue Parser::SyntaxError
      return
    end
  provider = Steep::Services::SignatureHelpProvider.new(source: source, subtyping: subtyping)
  items, index = provider.run(line: line, column: column)
  return unless items

  {
    "items" => items.map do |item|
      {
        "method_type" => item.method_type.to_s,
        "parameters" => item.parameters,
        "active_parameter" => item.active_parameter,
        "comment" => SteepServices.comment(item.comment)
      }
    end,
    "active_signature" => index
  }
end

SteepServices.each_query(ARGV) do |project, service, paths, directory|
  paths.each do |path|
    target = project.target_for_inline_source_path(path) || project.target_for_source_path(path) or next
    text = (project.base_dir + path).read
    SteepServices.query_positions(text).each do |line, column|
      result =
        begin
          signature_help(service, target, path, line, column)
        rescue => exception
          { "error" => exception.class.name }
        end
      SteepServices.print_result(directory, path, line, column, result) if result
    end
  end
end
