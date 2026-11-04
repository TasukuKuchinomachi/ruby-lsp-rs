module Describable
  #: () -> String
  def describe
    "described"
  end
end

class Product
  include Describable

  LIMIT = 10
  NAME = "product" #: String

  #: () -> Integer
  def limit
    LIMIT
  end
end

Product::LIMIT.no_such_method
Product::NAME.no_such_method
Product.new.describe.no_such_method
