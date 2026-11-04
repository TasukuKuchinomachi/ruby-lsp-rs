class Calculator
  def compute
    1
  rescue
    "rescued"
  else
    :else_value
  ensure
    2.0
  end
end
