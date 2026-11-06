class Account
  def initialize
    @balance = 0
  end

  def unsupported(value)
    case value
    in Integer
      value.no_such_method
    end
  end

  def defined_check
    defined?(no_such_method.more)
  end

  def splat(values)
    case 1
    when *values
      values.no_such_method
    end
  end

  def operator_assignments
    @@count ||= 0
    Account::LIMIT += 1
  end

  def multiple_assignment(values)
    @balance, self.name = values
  end

  def skipped
    __skip__ = 1.no_such_method
  end

  def name=(value)
    value
  end
end

object = [Account.new, nil].first
class << object
  def extra
    1.no_such_method
  end
end
