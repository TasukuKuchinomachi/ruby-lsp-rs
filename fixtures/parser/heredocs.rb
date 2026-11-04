plain = <<EOS
first line
  second line with #{interpolation} inside
EOS

dashed = <<-EOS
    dashed body
    EOS

squiggly = <<~EOS
  indented
    more indented

  after blank #{value}
EOS

starting_with_interpolation = <<~EOS
  #{first} and the rest
  second line
EOS

only_interpolation = <<~EOS
  #{only}
EOS

raw = <<~'EOS'
  no #{interpolation} here \n
  backslash \
  continued
EOS

escapes = <<~EOS
  tab\tnew\nline
  continued \
  line
EOS

empty = <<~EOS
EOS

two(<<~FIRST, <<~SECOND)
  first body
FIRST
  second body
SECOND

method_on_heredoc = <<~EOS.lines.map(&:strip)
  a
  b
EOS

command = <<~`SHELL`
  echo #{name}
  ls
SHELL

quoted = <<~"EOS"
  quoted #{x}
EOS

multibyte = <<~EOS
  日本語の行
  #{名前} さん
EOS

nested = <<~OUTER
  outer #{<<~INNER}
    inner
  INNER
  after
OUTER

call_with_block(<<~EOS) do |x|
  body
EOS
  x
end

dedent_tab = <<~EOS
	tab indented
	  more
EOS

trailing_space_terminator = <<-EOS
text
   EOS
