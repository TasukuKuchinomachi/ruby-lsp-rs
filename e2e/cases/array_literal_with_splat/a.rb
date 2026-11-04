numbers = [1, 2]
words = ["a"]

combined = [*numbers, *words, :symbol]
combined.no_such_method

range_items = [*1..3]
range_items.no_such_method

# @type var only_integers: Array[Integer]
only_integers = [0, *words]
