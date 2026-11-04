double = lambda { |x| x * 2 }
double.no_such_method

# @type var typed: ^(Integer) -> String
typed = lambda { |x| x.to_s }
typed = lambda { |x| x }

block = proc { |x| x }
block.no_such_method

stabby = ->(x) { x }
stabby.no_such_method
