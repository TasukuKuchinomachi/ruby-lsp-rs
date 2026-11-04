module Garage
  class Car
    def initialize(name)
      @name = name
    end

    def rename(name)
      @name = name
    end

    def label
      @name
    end
  end
end
