names = [] #: Array[String]
names << 1

loaded = Loader.new.load #: Integer
loaded.no_such_method

widened = 1 #: Integer | String
widened.no_such_method
