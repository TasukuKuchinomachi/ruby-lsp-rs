node = Node.new
parent_name = node.parent&.name
parent_name.no_such_method

node.parent&.no_such_method
node.parent.name

node&.name.no_such_method
