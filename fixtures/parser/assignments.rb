a = 1
a, b = 1, 2
a, b = b, a
a, = list
a, *b = list
*a, b = list
a, *, b = list
*, a = list
(a, b), c = list
a, (b, (c, d)) = list
((a, b)) = list
a, b = *list
a = *list
a = 1, 2
@a, @@b, $c, D, e.f, g[1] = list
A::B, ::C = list
a.b, c[d] = 1, 2
for x in list do end
for x, y in list; end
for (a, b) in list
end
a += 1
a -= 1
a *= 2
a **= 2
a ||= 1
a &&= 1
a <<= 1
@a += 1
@a ||= 1
@a &&= 1
@@a += 1
@@a ||= 1
@@a &&= 1
$a += 1
$a ||= 1
$a &&= 1
A += 1
A ||= 1
A &&= 1
A::B += 1
A::B ||= 1
A::B &&= 1
::C ||= 1
foo.bar += 1
foo.bar ||= 1
foo.bar &&= 1
foo&.bar += 1
foo&.bar ||= 1
foo::bar += 1
foo[1] += 1
foo[1] ||= 1
foo[1] &&= 1
foo[1, 2] += 3
foo[] ||= 1
foo.bar = 1
foo::bar = 1
foo.bar = 1, 2
A = 1
A::B = 1
::C = 1
a = b = c = 1
a = (b = 1)
a = begin; 1; end
a = if b then 1 else 2 end
a = 1 rescue nil
a = (raise rescue 2)
x = y rescue z
$1
$&
$`
$'
$+
$~
$stdout
@@cvar
@ivar
FROZEN = "value".freeze
