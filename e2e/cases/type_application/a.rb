Container.new.identity(1) #$ Integer
Container.new.identity(1) #$ String
Container.new.identity(nil) #$ String?
widened = Container.new.identity(1) #$ Integer | String
widened.no_such_method
