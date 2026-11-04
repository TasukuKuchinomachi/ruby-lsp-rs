# @rbs module-self: Array[Integer]
module Countable
  #: () -> Integer
  def total
    sum.no_such_method
    sum
  end
end

class Numbers
  include Countable
end

Collection = Array #: class-alias
Collection.new.no_such_method
