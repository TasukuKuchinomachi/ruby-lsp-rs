class Base
  #: () -> String
  def name
    "base"
  end
end

class Child < Base
  # @rbs skip
  def name
    1
  end

  # @rbs skip
  def skipped_method
    "skipped"
  end
end

Child.new.name.no_such_method
Child.new.skipped_method
