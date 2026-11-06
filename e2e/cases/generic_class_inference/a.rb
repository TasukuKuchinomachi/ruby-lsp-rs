box = Box.new(1)
box.get.no_such_method

mapped = box.map { |value| value.to_s }
mapped.get.no_such_method

# @type var strings: Box[String]
strings = Box.new(1)
