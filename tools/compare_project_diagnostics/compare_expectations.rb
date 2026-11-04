require "yaml"

FRESH_TYPE_VARIABLE = /\b([A-Z_a-z][A-Za-z0-9_]*)\((\d+)\)/

Diagnostic = Struct.new(:file, :start_line, :start_character, :end_line, :end_character, :code, :severity, :message, keyword_init: true) do
  def position
    [file, start_line, start_character, end_line, end_character, code]
  end

  def location
    "#{file}:#{start_line}:#{start_character}-#{end_line}:#{end_character}"
  end
end

def renumber_fresh_type_variables(message)
  numbers = {}
  message.gsub(FRESH_TYPE_VARIABLE) do
    name = Regexp.last_match(1)
    number = numbers[[name, Regexp.last_match(2)]] ||= numbers.count {|(other_name, _), _| other_name == name } + 1
    "#{name}(##{number})"
  end
end

def normalize_message(code, message, project_directory)
  message = message.gsub("#{project_directory}/", "PROJECT/") if project_directory
  message = message.lines.first.to_s.chomp if code == "Ruby::UnexpectedError"
  renumber_fresh_type_variables(message)
end

def load_diagnostics(path, project_directory)
  (YAML.safe_load(File.read(path)) || []).flat_map do |file|
    file.fetch("diagnostics").map do |diagnostic|
      range = diagnostic.fetch("range")
      Diagnostic.new(
        file: file.fetch("file"),
        start_line: range.dig("start", "line"),
        start_character: range.dig("start", "character"),
        end_line: range.dig("end", "line"),
        end_character: range.dig("end", "character"),
        code: diagnostic.fetch("code"),
        severity: diagnostic.fetch("severity"),
        message: normalize_message(diagnostic.fetch("code"), diagnostic.fetch("message"), project_directory)
      )
    end
  end
end

def same?(steep, steep_check)
  steep.severity == steep_check.severity && steep.message == steep_check.message
end

steep_path, steep_check_path, steep_project_directory, steep_check_project_directory = ARGV
abort "usage: #{$PROGRAM_NAME} STEEP_EXPECTATIONS STEEP_CHECK_EXPECTATIONS [STEEP_PROJECT_DIRECTORY STEEP_CHECK_PROJECT_DIRECTORY]" unless steep_path && steep_check_path

steep_by_position = load_diagnostics(steep_path, steep_project_directory).group_by(&:position)
steep_check_by_position = load_diagnostics(steep_check_path, steep_check_project_directory).group_by(&:position)

entries = []
(steep_by_position.keys | steep_check_by_position.keys).each do |position|
  steep_remaining = (steep_by_position[position] || []).dup
  steep_check_remaining = (steep_check_by_position[position] || []).dup
  steep_remaining.dup.each do |steep|
    index = steep_check_remaining.index {|steep_check| same?(steep, steep_check) }
    next unless index
    entries << [:same, steep, steep_check_remaining.delete_at(index)]
    steep_remaining.delete_at(steep_remaining.index(steep) || raise)
  end
  steep_remaining.zip(steep_check_remaining.shift(steep_remaining.size)).each do |steep, steep_check|
    entries << (steep_check ? [:differing_message, steep, steep_check] : [:only_in_steep, steep, nil])
  end
  steep_check_remaining.each {|steep_check| entries << [:only_in_steep_check, nil, steep_check] }
end

entries.sort_by! {|(_, steep, steep_check)| (steep || steep_check).position.map {|value| value.is_a?(Integer) ? format("%09d", value) : value } }

def print_message(prefix, diagnostic)
  lines = "[#{diagnostic.severity}] #{diagnostic.message}".lines(chomp: true)
  puts "  #{prefix} #{lines.first}"
  lines.drop(1).each {|line| puts "  #{prefix}   #{line}" }
end

labels = {
  same: "same",
  only_in_steep: "only in Steep",
  only_in_steep_check: "only in steep-check",
  differing_message: "differing message"
}

entries.each do |(kind, steep, steep_check)|
  next if kind == :same
  diagnostic = steep || steep_check
  puts "#{diagnostic.location} #{diagnostic.code} (#{labels.fetch(kind)})"
  print_message("-", steep) if steep
  print_message("+", steep_check) if steep_check
  puts
end

counts = entries.group_by(&:first).transform_values(&:size)
codes = entries.map {|(_, steep, steep_check)| (steep || steep_check).code }.uniq.sort
code_width = [codes.map(&:size).max || 0, "code".size].max
puts "Diagnostics by code:"
puts format("  %-#{code_width}s  %6s  %13s  %16s  %17s", "code", *labels.values)
codes.each do |code|
  by_kind = entries.select {|(_, steep, steep_check)| (steep || steep_check).code == code }.group_by(&:first).transform_values(&:size)
  puts format("  %-#{code_width}s  %6d  %13d  %16d  %17d", code, *labels.keys.map {|kind| by_kind.fetch(kind, 0) })
end
puts
puts "Summary: Steep reported #{steep_by_position.values.sum(&:size)}, steep-check reported #{steep_check_by_position.values.sum(&:size)}"
labels.each {|kind, label| puts "  #{label}: #{counts.fetch(kind, 0)}" }

exit(entries.all? {|(kind, _, _)| kind == :same } ? 0 : 1)
