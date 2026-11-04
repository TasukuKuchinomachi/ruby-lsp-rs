class Signature
  def all_kinds(required, optional = "x", *rest, name:, age: 1, **options, &block)
    required.no_such_method
    optional.no_such_method
    rest.no_such_method
    name.no_such_method
    age.no_such_method
    options.no_such_method
    block.no_such_method
  end
end

def undeclared_parameters(required, optional = 1, *rest, name:, age: "x", **options, &block)
  required.no_such_method
  optional.no_such_method
  rest.no_such_method
  age.no_such_method
  options.no_such_method
  block.no_such_method
end
