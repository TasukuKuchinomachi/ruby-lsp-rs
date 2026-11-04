class Registry
  @@entries = ["first"]

  def self.register(name)
    @@entries << name
    @@entries << 1
    @@entries = "wrong"
    @@unknown = 1
    @@unknown.no_such_method
  end
end
