receiver = Receiver.new
numbers = [1, 2]
strings = ["a", "b"]

receiver.positional(*numbers)
receiver.variadic(*numbers)
receiver.variadic(*strings)
receiver.variadic(1, *numbers, 3)

options = { name: "x" }
receiver.keywords(**options)

counts = { a: 1 }
receiver.any_keywords(**counts)
receiver.any_keywords(**options)
