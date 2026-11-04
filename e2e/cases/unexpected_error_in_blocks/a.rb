pairs = [[1, [2, 3]], [4, [5]]]

pairs.each do |(first, *rest)|
  first.foo
  rest.bar
end

pairs.each do |(first, *)|
  first.foo
end

total = pairs.map {|(first, *rest)| first + rest.size }
total.baz

x = 1
x.baz

Runner.new.run do |first, second|
  first.foo
end

Runner.new.run do |value|
  # @implements Unknown
  value.foo
end
