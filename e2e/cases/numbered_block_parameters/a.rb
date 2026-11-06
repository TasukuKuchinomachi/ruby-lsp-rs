[1, 2].map { _1.no_such_method }
{ a: 1 }.each { _1.no_such_method; _2.no_such_method }

# @type var strings: Array[String]
strings = [1, 2].map { _1 + 1 }
