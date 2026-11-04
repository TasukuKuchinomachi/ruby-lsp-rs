steep_version_argument = ARGV.find { |argument| argument.start_with?("--steep-version=") }
steep_version = steep_version_argument ? ARGV.delete(steep_version_argument).delete_prefix("--steep-version=") : "2.1.0"
gem "steep", steep_version
require "steep"

MAP_FIELDS = %w[
  expression keyword name selector operator begin end dot else assignment
  question colon double_colon in assoc argument heredoc_body heredoc_end
].freeze

def escape(string)
  string.b.each_byte.map do |byte|
    case byte
    when 0x5c then "\\\\"
    when 0x22 then "\\\""
    when 0x00..0x1f, 0x7f then format("\\x%02X", byte)
    else byte.chr
    end
  end.join.force_encoding(Encoding::UTF_8)
end

def range(range)
  return "nil" unless range

  "#{range.line}:#{range.column}-#{range.last_line}:#{range.last_column}"
end

def map(location)
  return "nil" unless location

  fields = MAP_FIELDS.select { |field| location.respond_to?(field) }
  [location.class.name.split("::").last, *fields.map { |field| "#{field}=#{range(location.public_send(field))}" }].join(" ")
end

def child(value, indent, output)
  case value
  when Parser::AST::Node then node(value, indent, output)
  when nil then output << "#{indent}nil"
  when Symbol then output << "#{indent}:\"#{escape(value.to_s)}\""
  when String then output << "#{indent}\"#{escape(value)}\""
  when Integer then output << "#{indent}integer #{value}"
  when Float then output << "#{indent}float #{value.inspect}"
  when Rational then output << "#{indent}rational #{value.inspect}"
  when Complex then output << "#{indent}complex #{value.inspect}"
  else raise "unexpected child: #{value.inspect}"
  end
end

def node(node, indent, output)
  output << "#{indent}(#{node.type} #{map(node.location)}"
  node.children.each { |value| child(value, "#{indent}  ", output) }
end

ARGV.each do |path|
  output = ["file #{path}"]
  buffer = Parser::Source::Buffer.new(path, 1, source: File.read(path))
  begin
    root, comments = Steep::Source.new_parser.parse_with_comments(buffer)
    root ? node(root, "", output) : output << "nil"
    comments.each do |comment|
      output << "comment #{range(comment.location.expression)} \"#{escape(comment.text)}\""
    end
  rescue Parser::SyntaxError => error
    output << "error #{range(error.diagnostic.location)} \"#{escape(error.message)}\""
  end
  puts output
end
