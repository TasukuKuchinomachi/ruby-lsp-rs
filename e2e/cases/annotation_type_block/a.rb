[1, 2].each do |number|
  # @type block: String
  number
end

Runner.new.run do |number|
  # @type block: String
  number.to_s
end
