# @type var klass: singleton(Dog) | singleton(Car)
klass = Dog

if klass < Animal
  klass.no_such_method
else
  klass.no_such_method
end

if klass <= Car
  klass.no_such_method
end
