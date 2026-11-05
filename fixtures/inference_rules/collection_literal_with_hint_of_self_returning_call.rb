class Settings
  ENTRIES = [{ name: "a" }].freeze

  def label
    key = :a
    label = "b"
    { key:, label: }.freeze
  end

  def pair
    [1, "a"].freeze
  end
end
