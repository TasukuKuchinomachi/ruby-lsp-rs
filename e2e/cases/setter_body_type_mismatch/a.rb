class Profile
  def name=(value)
    @name = value
    value.to_sym
  end
end
