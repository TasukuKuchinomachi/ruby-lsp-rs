BEGIN { setup }
END { teardown }

if a
  1
elsif b
  2
elsif c then 3
else
  4
end
if a then 1 end
if a; 1; end
if a
end
unless a
  1
else
  2
end
unless a then 1 end
x = a ? b : c
puts 1 if a
puts 1 unless a
while a
  break
end
while a do next end
until a; redo; end
puts 1 while a
puts 1 until a
begin
  work
end while a
begin
  work
end until a
loop do
  break 1
  next 2, 3
end
case x
when 1, 2 then :a
when *list
  :b
when 3; :c
else
  :d
end
case
when a then 1
end
case x
when 1
end
begin
  work
rescue ArgumentError, TypeError => e
  retry
rescue => e
  e
rescue
  nil
else
  done
ensure
  cleanup
end
begin
  work
ensure
end
begin
end
begin
  1
  2
end
begin; rescue; end
begin
  work
rescue Error; handle
end
begin
  work
rescue Error then handle
end
def with_rescue
  work
rescue StandardError
  nil
ensure
  cleanup
end
call do
  work
rescue
  retry
end
defined?(a)
defined? a
defined?(@a)
defined?(
  a
)
defined?
(a)
if (a == 1)..(b == 2)
  flip
end
if (a == 1)...(b == 2)
  flop
end
if a..b then 1 end
x = !(a .. b)
1..2
1...2
1..
..2
(1..)
x = (..2)
a and b or not c
a && b || !c
a && (b || c)
return
return 1
return 1, 2
return *list
x = __FILE__
y = __LINE__
z = __ENCODING__
alias new_name old_name
alias :new_sym :old_sym
alias $new $old
alias + -
undef foo, :bar, :"baz"
class Foo < Bar
  def method; end
end
class Foo::Bar < ::Baz; end
class << self
  def sclass_method; end
end
module Mod
  module Nested; end
end
class Empty; end
0x1F
0b1010
0o17
017
1_000_000
1.5
1e10
1.5e-3
1e100
1e16
1e15
1.0e-5
0.0001
123456789012345678901234567890
-123456789012345678901234567890
1r
1.5r
-2r
3i
1.5i
2ri
-3i
-1.5ri
-1
+1
-1.5
+1.5
-0
0.0
1e400
-1e400
{ a: 1, "b" => 2, c => 3, **d }
{ "a": 1 }
{ a: 1, }
{}
[1, *a, 2]
[]
[1, [2, 3]]
p(a: 1)
x = [1, 2,
  3]
x
  .foo
  .bar
x.foo
  &.bar
a = 1; b = 2
x = (1; 2)
x = ()
puts(())
__END__
this is data
not parsed
