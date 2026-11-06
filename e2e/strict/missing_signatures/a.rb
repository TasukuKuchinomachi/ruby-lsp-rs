class Inventory
  def initialize
  end

  def empty_collections
    items = Array.new
    table = Hash.new
    items.no_such_method
    table.no_such_method
  end

  def annotated_collections
    Array.new
  end

  def constrained
    Array.new(3, 0)
  end

  def untyped_parameters(count, limit = 1)
    count.no_such_method
    limit
  end

  def untyped_overload(count)
    count.no_such_method
  end

  def untyped_without_parameters
  end

  def untyped_keywords(*rest, key:, optional: 1, **options, &block)
    rest.no_such_method
    key.no_such_method
    optional.no_such_method
    options.no_such_method
    block.no_such_method
  end

  def call_untyped
    untyped_parameters(1.no_such_method, "a")
    untyped_without_parameters
  end

  def yield_untyped
    value = yield 1, 2
    value.no_such_method
  end

  def yield_untyped_block
    yield "a", :b
  end

  def yield_typed
    yield 1
  end

  def type_arguments
    generic(1) #$ Integer, String
  end

  def forward(...)
    untyped_parameters(...)
  end

  def annotated_parameter(count, other)
    # @type var count: Integer
    count.no_such_method
  end

  def assigned_to_declared
    @items = Array.new
    nil
  end

  def generic(value)
    value
  end

  def propagated(value)
    first = 1.yield_self { value }
    second = Array(value)
    first.no_such_method
    second.no_such_method
  end
end
