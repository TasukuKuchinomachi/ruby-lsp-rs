class Counter
  def initialize
    @count = 0
    @label = nil
  end

  def increment
    @count + 1
  end

  def rename
    @label = "counter"
  end

  def label
    @label
  end

  def self.cache
    @cache = [1]
    @cache
  end
end
