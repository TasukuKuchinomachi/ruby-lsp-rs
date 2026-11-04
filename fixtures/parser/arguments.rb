def required(a, b); end
def optional(a, b = 1, c = foo); end
def rest(*args); end
def anonymous_rest(*); other(*); end
def posts(a, *b, c, d); end
def keywords(a:, b: 1, **opts); end
def anonymous_keyword_rest(**); other(**); end
def no_keywords(**nil); end
def block(&blk); end
def anonymous_block(&); other(&); end
def forwarding(...); other(...); end
def leading_forwarding(a, ...); other(a, ...); end
def everything(a, b = 1, *c, d, e:, f: 2, **g, &h); end
def destructuring((a, b), (c, *d)); end
def nested_destructuring((a, (b, c))); end
def no_parens a, b
end
def self.singleton(a) = a + 1
def endless(a) = a * 2
def endless_command(a) = puts a
def setter=(value); end
def ==(other); end
def +@; end
def [](index); end
def []=(index, value); end
def `(command); end
def !; end
def (expression).on_expression; end
def obj.on_object; end
def Const.on_constant; end

call(1, 2)
call 1, 2
call(*args)
call(**opts)
call(&blk)
call(a, *b, c, **d, &e)
call(key: 1, other: 2)
call(:sym => 1, "str" => 2)
call({ key: 1 })
call(1, key: 1, &blk)
call(a:, b:)
call(A:)
call key: 1 do |x| end
call() { |a| }
call { |a, | }
call { |a, b| }
call { |*a| }
call { |a, *b, c| }
call { |(a, b), c| }
call { |(a, b)| }
call { |a; b, c| }
call { |;b| }
call { |a = 1| }
call { |k:| }
call { |**k| }
call { |&b| }
call { _1 + _2 }
call { it }
call { |a| a }
call do |a| end
-> {}
-> { 1 }
->(a) { a }
->(a, b = 1, *c, d:, e: 2, **f, &g) {}
-> a { a }
->(a; b) {}
-> { _1 }
-> { it }
lambda { |x| x }
proc { |x, y| }
obj.call(1) { |x| x }
obj&.call(1) { |x| x }
super
super()
super(1, 2)
super(1) { |x| x }
super { |x| x }
super(&blk)
super(key: 1)
yield
yield 1, 2
yield(key: 1)
foo.()
foo.(1)
foo::bar
foo::Bar
foo::bar(1)
a.b.c(1).d { }.e
a&.b
a&.b(1)
a&.b = 1
!a
not a
not(a)
not()
-a
+a
~a
-2 ** 2
- 1
-1.abs
a.!
a.-@
foo[1]
foo[1, 2]
foo[]
foo[key: 1]
foo[*a]
foo[1] = 2
foo[1, 2] = 3
foo.[](1)
foo.[]=(1, 2)
foo&.[](1)
