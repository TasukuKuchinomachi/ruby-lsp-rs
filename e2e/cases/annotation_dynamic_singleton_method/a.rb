class Config
  # @dynamic self.load
  define_singleton_method(:load) { new }

  # @dynamic self.missing
end
