# @type var count: Integer
count = 1

# @type variable name: String
name = "x"

# @type var Label: String
label = 1

# @dynamic
# @implements
# @type var total: Integer
total = 0

numbers = [] #: Array[Integer
letters = [] #: Array[String]

#:nodoc:
# :ready, :failed
helpers = [count, name, label, total, numbers, letters]

# steep:ignore:start
ignored = helpers #: Array[
# steep:ignore:end
