pinned = 1
case value
in Integer | Float => number
  number
in [1, 2, *rest]
  rest
in [first, *, last]
  first
in [*, middle, *]
  middle
in [*pre, 1, *post]
  pre
in { name: String => name, age: }
  age
in { "quoted": 1, "other": }
  other
in { key: nil, **rest }
  rest
in { only: 1, **nil }
  nil
in {}
  nil
in []
  nil
in Point(x:, y:)
  x
in Point[x, y]
  y
in Point()
  nil
in Point[]
  nil
in ^pinned
  nil
in ^(1 + 2)
  nil
in ^@ivar
  nil
in 1..10
  nil
in ..5
  nil
in 5..
  nil
in nil
  nil
in :sym | "str" | /re/
  nil
in x if x > 1
  x
in y unless y < 1
  y
in a, b
  a
in c, then c
in [a, [b, c]]
  b
in { a: { b: } }
  b
in String then "then"
in -> x { x }
  nil
in Foo::Bar
  nil
in ::Baz
  nil
in _
  nil
end

case value
in 1
else
end

case value
in 1 then nil
else
  2
end

value => { name: }
value => [x, y]
value in Integer
value in [a, *]
result = value in { key: }
value => _unused
