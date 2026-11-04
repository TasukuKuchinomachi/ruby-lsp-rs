# @type var strings: Array[String]
strings = [1, 2, 3].map do |number|
  next "small" if number < 2
  next number if number < 3
  "large"
end

[1, 2].each do |number|
  next
end
