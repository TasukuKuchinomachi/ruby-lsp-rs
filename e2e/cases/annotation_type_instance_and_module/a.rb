class Service
  # @type instance: Special
  # @type module: singleton(Special)

  def call
    special
    no_such_method
  end

  def self.create
    new
  end
end
