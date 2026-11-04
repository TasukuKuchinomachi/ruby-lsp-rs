class Greeter
  def hello
    "hello"
  end

  alias greet hello
  alias undeclared_alias hello
end

Greeter.new.greet.no_such_method
alias $new_global $stdout
