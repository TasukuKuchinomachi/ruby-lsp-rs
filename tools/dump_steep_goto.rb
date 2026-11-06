require_relative "steep_services"

def locations(goto, kind, path, line, column)
  goto.public_send(kind, path: path, line: line, column: column).map { |location| SteepServices.location(location) }.sort
rescue => exception
  { "error" => exception.class.name }
end

SteepServices.each_query(ARGV) do |project, service, paths, directory|
  paths.each do |path|
    target = project.target_for_inline_source_path(path) || project.target_for_source_path(path)
    service.typecheck_source(path: path, target: target) if target
  end
  goto = Steep::Services::GotoService.new(type_check: service, assignment: Steep::Services::PathAssignment.all)
  paths.each do |path|
    text = (project.base_dir + path).read
    SteepServices.query_positions(text).each do |line, column|
      result = %i[definition implementation type_definition].to_h do |kind|
        [kind.to_s, locations(goto, kind, path, line, column)]
      end
      next if result.each_value.all? { |value| value.is_a?(Array) && value.empty? }

      SteepServices.print_result(directory, path, line, column, result)
    end
  end
end
