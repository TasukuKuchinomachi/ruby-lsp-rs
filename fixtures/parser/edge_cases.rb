text = <<~EOS
  first #{a} middle #{b}
    indented #{c}
  plain
EOS
hash = { key: <<~EOS, other: 1 }
  in hash
EOS
words = %w[a\
b c]
symbols = %i[a\
b c]
interpolated_words = %W[a\
b #{c}]
continued = "first\
second\
third"
escaped_newline = "a\\
b"
double_quoted_newlines = "a\nb
c\nd"
single_quoted = 'a\nb
c'
xstring = `echo a
b`
symbol = :"multi
line"
regexp = /a
b/
percent_q = %q{a
b}
interpolated_symbol = :"a#{b}
c"
number_one = 1.0e20i
number_two = -0.0
number_three = 1e-4
number_four = 9999999999999998.0
number_five = 0x7fffffffffffffffffff
number_six = 0.1r
number_seven = 1/3r
character_escape = ?\t
character_unicode = ?A
lambda_with_block_local = ->(a; b) { a }
string_in_condition = "a" if "b"
regexp_in_unless = 1 unless /x/
not_regexp = !/x/
while /y/ do end
negated_flip_flop = 1 if !(a..b)
begin_end_while = begin; 1; end while false
ternary_nested = a ? b ? 1 : 2 : 3
case_without_else = case a when 1 then 2 end
safe_navigation_call = a&.b&.c(1)&.d { }
operator_call = a + b * c - d / e % f ** g
comparison = a <=> b
bitwise = a & b | c ^ d << 1 >> 2
equality = a == b && a != c && a === d && a =~ /e/ && a !~ /f/
unary = -a + +b - ~c
index_with_block = a[1] { }
call_with_trailing_comma = foo(1, 2,)
array_with_splat_and_kwsplat = [*a, **b]
nested_hash = { a: { b: { c: 1 } } }
string_key_hash = { "a" => 1, :b => 2, 3 => 4, nil => 5 }
method_reference = method(:foo)
block_pass_symbol = list.map(&:to_s)
block_pass_method = list.each(&method(:puts))
multiple_return = def m = (return 1, 2)
attribute_and_operator = a.b += c.d -= 1
