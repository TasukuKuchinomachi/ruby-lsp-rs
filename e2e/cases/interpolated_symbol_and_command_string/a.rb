name = "world"
symbol = :"hello_#{name}"
symbol.no_such_method

output = `echo #{name}`
output.no_such_method

plain_command = %x(date)
plain_command.no_such_method

heredoc = <<~TEXT
  Hello #{name.no_such_method}
TEXT
heredoc.no_such_method
