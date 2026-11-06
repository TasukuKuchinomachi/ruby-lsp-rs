class Sample
  def run
    # @type var text: String
    text = 1
    [1].first.no_such_method
    UnknownConstant
    [1].size { 2 }
    @undeclared_instance_variable = 1
    empty = []
    "not an integer"
  end

  def undeclared
  end
end
