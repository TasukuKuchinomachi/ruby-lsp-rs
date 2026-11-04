maker = PairMaker.new
hash = maker.make {|string| [string, string.size] }
hash.each_key {|key| key.upcase }
hash.each_value {|value| value + 1 }

maker.make_pair {|string| [string.to_sym, string.size] }

maker.make_strings {|string| [string, string.size] }

pairs = [[1, "one"]].to_h
pairs.each {|number, name| number + name.size }
