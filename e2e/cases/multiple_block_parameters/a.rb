# @type var pairs: Array[[Integer, String]]
pairs = []
pairs.each do |(a, b)|
  a.no_such_method
  b.no_such_method
end

# @type var nested: Array[[Integer, [String, Symbol]]]
nested = []
nested.each do |x, (y, z)|
  x.no_such_method
  y.no_such_method
  z.no_such_method
end
nested.each_with_index do |(x, (y, z)), i|
  x.no_such_method
  y.no_such_method
  z.no_such_method
  i.no_such_method
end

# @type var deep: Array[[[Integer, String], Symbol]]
deep = []
deep.each do |((a, b), c)|
  a.no_such_method
  b.no_such_method
  c.no_such_method
end

# @type var hash: Hash[Symbol, Integer]
hash = {}
hash.each do |(key, value)|
  key.no_such_method
  value.no_such_method
end
hash.each_with_object([]) do |(key, value), memo|
  key.no_such_method
  value.no_such_method
  memo.no_such_method
end

# @type var lists: Hash[Symbol, Array[String]]
lists = {}
lists.each do |key, (first, second)|
  first.no_such_method
  second.no_such_method
end

# @type var integers: Array[Integer]
integers = []
integers.each do |(a, b)|
  a.no_such_method
  b.no_such_method
end

# @type var unions: Array[[Integer, String] | [Symbol]]
unions = []
unions.each do |(a, b)|
  a.no_such_method
  b.no_such_method
end

# @type var convertibles: Array[Convertible]
convertibles = []
convertibles.each do |(a, b)|
  a.no_such_method
  b.no_such_method
end

# @type var broken: Array[BrokenConvertible]
broken = []
broken.each do |(a, b)|
  # @type var a: Integer
  a.no_such_method
  b.no_such_method
end

# @type var aliased: Array[pair]
aliased = []
aliased.each do |(a, b)|
  a.no_such_method
  b.no_such_method
end

# @type var untyped_values: Array[untyped]
untyped_values = []
untyped_values.each do |(a, b)|
  a.no_such_method
  b.no_such_method
end

# @type var optional_pairs: Array[[Integer, String]?]
optional_pairs = []
optional_pairs.each do |(a, b)|
  a.no_such_method
  b.no_such_method
end

Generic.new.pick(1, :symbol)
