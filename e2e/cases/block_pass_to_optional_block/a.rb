def stringify(value)
  value.to_s
end

test = OptionalBlockTest.new

test.sum(&:to_r)
test.sum(0, &:to_r)
test.map(&:to_s)
test.map(&method(:stringify))
test.map(&:no_such_method)
test.map(&nil)

test.map
test.map {|element| element.to_s }
