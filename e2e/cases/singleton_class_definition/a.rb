class Factory
  class << self
    def build
      new
    end

    def name_of(factory)
      factory
    end

    def undeclared_singleton_method
      self.no_such_method
    end
  end
end
